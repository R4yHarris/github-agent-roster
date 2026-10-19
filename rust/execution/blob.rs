use std::ops::Range;

use super::{MAX_FILE_BYTES, MAX_FILES, MAX_SNAPSHOT_BYTES, denied};
use crate::Error;

const FRAME_BYTES: usize = 80;
const HEADER_BYTES: usize = FRAME_BYTES - 1;
const STDERR_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub(crate) struct Object {
    oid: String,
    size: usize,
}

impl Object {
    pub(crate) fn new(oid: &str, size: u64) -> Result<Self, Error> {
        if !matches!(oid.len(), 40 | 64)
            || !oid
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || size > MAX_FILE_BYTES
        {
            return Err(denied(
                "Blob request requires a full object ID and at most 16 MiB",
            ));
        }
        Ok(Self {
            oid: oid.to_owned(),
            size: usize::try_from(size).map_err(|_| denied("Blob size must fit usize"))?,
        })
    }

    pub(crate) fn oid(&self) -> &str {
        &self.oid
    }
    pub(crate) fn size(&self) -> usize {
        self.size
    }
}

pub(crate) fn size(bytes: &[u8]) -> Result<u64, Error> {
    if bytes.is_empty() || (bytes.len() > 1 && bytes.first() == Some(&b'0')) {
        return Err(denied("Blob size must be a canonical nonnegative decimal"));
    }
    bytes.iter().try_fold(0_u64, |value, byte| {
        if !byte.is_ascii_digit() {
            return Err(denied("Blob size must be decimal"));
        }
        value
            .checked_mul(10)
            .and_then(|value| value.checked_add(u64::from(*byte - b'0')))
            .ok_or_else(|| denied("Blob size decimal overflows u64"))
    })
}

pub(crate) fn chunks(objects: &[Object]) -> Result<Vec<Range<usize>>, Error> {
    if objects.len() > MAX_FILES {
        return Err(denied("Blob plan exceeds 2048 entries"));
    }
    let mut result = Vec::new();
    let mut start = 0;
    let mut used = 0_u64;
    let mut total = 0_u64;
    for (index, object) in objects.iter().enumerate() {
        let bytes = u64::try_from(object.size()).map_err(|_| denied("Blob size exceeds u64"))?;
        total = total
            .checked_add(bytes)
            .ok_or_else(|| denied("Base byte count overflow"))?;
        if bytes > MAX_FILE_BYTES || total > MAX_SNAPSHOT_BYTES {
            return Err(denied(
                "Base revision exceeds its 16 MiB blob or local 128 MiB bound",
            ));
        }
        if used
            .checked_add(bytes)
            .is_none_or(|sum| sum > MAX_FILE_BYTES)
        {
            result.push(start..index);
            start = index;
            used = 0;
        }
        used = used
            .checked_add(bytes)
            .ok_or_else(|| denied("Blob chunk size overflow"))?;
    }
    if start < objects.len() {
        result.push(start..objects.len());
    }
    Ok(result)
}

pub(crate) fn request(objects: &[Object]) -> Result<(Vec<u8>, usize), Error> {
    if objects.is_empty() || objects.len() > MAX_FILES {
        return Err(denied("Blob chunk requires between one and 2048 objects"));
    }
    let payload = objects.iter().try_fold(0_usize, |sum, object| {
        sum.checked_add(object.size())
            .ok_or_else(|| denied("Blob chunk size overflow"))
    })?;
    if !u64::try_from(payload).is_ok_and(|bytes| bytes <= MAX_FILE_BYTES) {
        return Err(denied("Blob chunk payload exceeds 16 MiB"));
    }
    let maximum = objects
        .len()
        .checked_mul(FRAME_BYTES)
        .and_then(|frames| payload.checked_add(frames))
        .and_then(|stdout| stdout.checked_add(STDERR_BYTES))
        .filter(|maximum| *maximum <= 32 * 1024 * 1024)
        .ok_or_else(|| denied("Blob chunk output bound overflows"))?;
    let mut request = Vec::new();
    for object in objects {
        request.extend_from_slice(object.oid().as_bytes());
        request.push(b'\n');
    }
    if request.len() > 65 * MAX_FILES {
        return Err(denied("Blob object request exceeds its finite input bound"));
    }
    Ok((request, maximum))
}

pub(crate) struct Output {
    bytes: Vec<u8>,
    bodies: Vec<Range<usize>>,
}

impl Output {
    pub(crate) fn parse(bytes: Vec<u8>, objects: &[Object]) -> Result<Self, Error> {
        let (_, maximum) = request(objects)?;
        if bytes.len() > maximum {
            return Err(denied("Blob chunk output exceeds its bound"));
        }
        let mut cursor = 0_usize;
        let mut bodies = Vec::new();
        for object in objects {
            let limit = cursor
                .checked_add(HEADER_BYTES)
                .ok_or_else(|| denied("Blob header offset overflows"))?
                .min(bytes.len());
            let end = bytes
                .get(cursor..limit)
                .and_then(|header| header.iter().position(|byte| *byte == b'\n'))
                .and_then(|offset| cursor.checked_add(offset))
                .ok_or_else(|| denied("Blob batch header is missing, truncated or oversized"))?;
            let header = bytes
                .get(cursor..end)
                .ok_or_else(|| denied("Blob header range is invalid"))?;
            let mut fields = header.split(|byte| *byte == b' ');
            let [Some(oid), Some(kind), Some(length)] =
                [fields.next(), fields.next(), fields.next()]
            else {
                return Err(denied("Blob batch response requires object, type and size"));
            };
            if fields.next().is_some()
                || oid != object.oid.as_bytes()
                || kind != b"blob"
                || size(length)?
                    != u64::try_from(object.size)
                        .map_err(|_| denied("Blob plan size exceeds u64"))?
            {
                return Err(denied(
                    "Blob batch object, type or size does not match its ordinal plan",
                ));
            }
            let start = end
                .checked_add(1)
                .ok_or_else(|| denied("Blob body offset overflows"))?;
            let end = start
                .checked_add(object.size)
                .ok_or_else(|| denied("Blob body size overflows"))?;
            if bytes.get(start..end).is_none() || bytes.get(end) != Some(&b'\n') {
                return Err(denied("Blob body or terminating LF is missing"));
            }
            bodies.push(start..end);
            cursor = end
                .checked_add(1)
                .ok_or_else(|| denied("Blob response offset overflows"))?;
        }
        if cursor != bytes.len() {
            return Err(denied("Blob batch output contains extra data"));
        }
        Ok(Self { bytes, bodies })
    }

    pub(crate) fn body(&self, index: usize) -> Result<&[u8], Error> {
        self.bodies
            .get(index)
            .and_then(|range| self.bytes.get(range.clone()))
            .ok_or_else(|| denied("Blob ordinal body is unavailable"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(width: usize, size: u64) -> Result<Object, Error> {
        Object::new(&"a".repeat(width), size)
    }
    fn frame(object: &Object, body: &[u8]) -> Vec<u8> {
        let mut bytes = format!("{} blob {}\n", object.oid, object.size).into_bytes();
        bytes.extend_from_slice(body);
        bytes.push(b'\n');
        bytes
    }

    #[test]
    fn blob_frames_preserve_binary_empty_duplicate_order_and_both_hashes() -> Result<(), Error> {
        for width in [40, 64] {
            let body = [0, b'\n', 255, b'\r', 0];
            let value = object(width, 5)?;
            let empty = object(width, 0)?;
            let mut bytes = frame(&value, &body);
            bytes.extend(frame(&empty, &[]));
            bytes.extend(frame(&value, &body));
            let observed = Output::parse(bytes, &[value.clone(), empty, value])?;
            assert!(
                observed.body(0)? == body
                    && observed.body(1)?.is_empty()
                    && observed.body(2)? == body
            );
            assert!(observed.body(3).is_err());
        }
        Ok(())
    }

    #[test]
    fn blob_frames_reject_wrong_ordinal_type_size_missing_truncated_extra_and_encoding()
    -> Result<(), Error> {
        let value = object(40, 1)?;
        for bytes in [
            Vec::new(),
            format!("{} missing\n", value.oid).into_bytes(),
            format!("{} tree 1\nx\n", value.oid).into_bytes(),
            format!("{} blob 2\nxx\n", value.oid).into_bytes(),
            format!("{} blob 01\nx\n", value.oid).into_bytes(),
            format!("{} blob -1\nx\n", value.oid).into_bytes(),
            format!("{} blob 1 extra\nx\n", value.oid).into_bytes(),
            format!("{} blob 1\n", value.oid).into_bytes(),
            format!("{} blob 1\nx", value.oid).into_bytes(),
            format!("{} blob 1\nx\nextra", value.oid).into_bytes(),
            format!("{} blob 1\nx\r\n", value.oid).into_bytes(),
            b"bad\xff blob 1\nx\n".to_vec(),
            format!("{} blob {}\nx\n", value.oid, "9".repeat(200)).into_bytes(),
            format!("{} blob 1\nx\n", "b".repeat(40)).into_bytes(),
        ] {
            assert!(Output::parse(bytes, std::slice::from_ref(&value)).is_err());
        }
        Ok(())
    }

    #[test]
    fn blob_size_and_oid_validation_is_checked_and_canonical() {
        for value in [
            b"".as_slice(),
            b"+1",
            b"-1",
            b"01",
            b"1 ",
            b"18446744073709551616",
            b"\xff",
        ] {
            assert!(size(value).is_err());
        }
        assert_eq!(size(b"18446744073709551615").ok(), Some(u64::MAX));
        for oid in [
            "a".repeat(39),
            "a".repeat(41),
            "a".repeat(63),
            "a".repeat(65),
            "A".repeat(40),
            "g".repeat(40),
        ] {
            assert!(Object::new(&oid, 0).is_err());
        }
        assert!(Object::new(&"a".repeat(40), MAX_FILE_BYTES + 1).is_err());
    }

    #[test]
    fn blob_chunk_limits_cover_full_sixteen_mib_plus_protocol_and_local_128_mib()
    -> Result<(), Error> {
        let value = object(64, MAX_FILE_BYTES)?;
        let objects = vec![value.clone(); 8];
        assert_eq!(
            chunks(&objects)?,
            (0..8).map(|index| index..index + 1).collect::<Vec<_>>()
        );
        assert!(chunks(&vec![value.clone(); 9]).is_err());
        assert!(chunks(&[]).is_ok_and(|chunks| chunks.is_empty()));
        let zero = object(64, 0)?;
        let objects = vec![zero.clone(); MAX_FILES];
        assert_eq!(chunks(&objects)?, vec![0..MAX_FILES]);
        let (input, maximum) = request(&objects)?;
        assert_eq!(input.len(), 65 * MAX_FILES);
        assert_eq!(maximum, 80 * MAX_FILES + STDERR_BYTES);
        assert!(chunks(&vec![zero; MAX_FILES + 1]).is_err());
        assert!(request(&[]).is_err());
        let (input, maximum) = request(std::slice::from_ref(&value))?;
        assert_eq!(input.len(), 65);
        assert_eq!(maximum, 16 * 1024 * 1024 + 80 + STDERR_BYTES);
        let body = vec![0; 16 * 1024 * 1024];
        let observed = Output::parse(frame(&value, &body), std::slice::from_ref(&value))?;
        assert!(observed.body(0)? == body);
        Ok(())
    }

    #[test]
    fn blob_chunks_preserve_order_and_count_zero_objects_at_payload_edges() -> Result<(), Error> {
        let objects = [
            object(40, MAX_FILE_BYTES)?,
            object(40, 0)?,
            object(40, 1)?,
            object(40, 0)?,
        ];
        assert_eq!(chunks(&objects)?, vec![0..2, 2..4]);
        assert!(request(&objects).is_err());
        let mut maximum_objects = vec![object(64, 0)?; MAX_FILES - 1];
        maximum_objects.push(object(64, MAX_FILE_BYTES)?);
        let maximum = request(&maximum_objects)?.1;
        assert_eq!(maximum, 17_989_632);
        Ok(())
    }
}
