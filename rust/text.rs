use crate::Error;

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Text(pub Vec<u16>);

impl From<&str> for Text {
    fn from(value: &str) -> Self {
        Self(value.encode_utf16().collect())
    }
}

impl Text {
    pub fn utf8_len(&self) -> usize {
        char::decode_utf16(self.0.iter().copied())
            .map(|unit| match unit {
                Ok(character) => character.len_utf8(),
                Err(_) => 3,
            })
            .sum()
    }

    pub fn json(&self) -> String {
        let mut output = String::with_capacity(self.0.len() + 2);
        output.push('"');
        for unit in char::decode_utf16(self.0.iter().copied()) {
            match unit {
                Ok('"') => output.push_str("\\\""),
                Ok('\\') => output.push_str("\\\\"),
                Ok('\u{0008}') => output.push_str("\\b"),
                Ok('\u{000c}') => output.push_str("\\f"),
                Ok('\n') => output.push_str("\\n"),
                Ok('\r') => output.push_str("\\r"),
                Ok('\t') => output.push_str("\\t"),
                Ok(character) if character < ' ' => escape_unit(&mut output, character as u16),
                Ok(character) => output.push(character),
                Err(error) => escape_unit(&mut output, error.unpaired_surrogate()),
            }
        }
        output.push('"');
        output
    }

    pub fn normalized_lines(&self) -> Self {
        let mut output = Vec::with_capacity(self.0.len());
        let mut units = self.0.iter().peekable();
        while let Some(&unit) = units.next() {
            if unit == 13 && units.peek() == Some(&&10) {
                continue;
            }
            output.push(unit);
        }
        Self(output)
    }

    pub fn push_str(&mut self, value: &str) {
        self.0.extend(value.encode_utf16());
    }

    pub fn to_utf8(&self) -> Result<String, Error> {
        String::from_utf16(&self.0)
            .map_err(|_| Error::type_error("Text contains an unpaired UTF-16 surrogate"))
    }
}

fn escape_unit(output: &mut String, unit: u16) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    output.push_str("\\u");
    for shift in [12, 8, 4, 0] {
        output.push(HEX[((unit >> shift) & 15) as usize] as char);
    }
}

pub fn is_space(unit: u16) -> bool {
    matches!(
        unit,
        0x0009..=0x000d
            | 0x0020
            | 0x00a0
            | 0x1680
            | 0x2000..=0x200a
            | 0x2028
            | 0x2029
            | 0x202f
            | 0x205f
            | 0x3000
            | 0xfeff
    )
}

pub fn trim_end(mut units: &[u16]) -> &[u16] {
    while units.last().is_some_and(|unit| is_space(*unit)) {
        units = &units[..units.len() - 1];
    }
    units
}

pub fn trim(units: &[u16]) -> &[u16] {
    let units = trim_end(units);
    let first = units
        .iter()
        .position(|unit| !is_space(*unit))
        .unwrap_or(units.len());
    &units[first..]
}

pub fn ascii(units: &[u16], value: &str) -> bool {
    units.iter().copied().eq(value.encode_utf16())
}

pub fn parse_json_string(units: &[u16]) -> Result<Text, Error> {
    let invalid = || Error::type_error("Invalid JSON string");
    if units.first() != Some(&34) {
        return Err(invalid());
    }
    let mut value = Vec::with_capacity(units.len().saturating_sub(2));
    let mut index = 1;
    while let Some(&unit) = units.get(index) {
        index += 1;
        match unit {
            34 => {
                if units[index..]
                    .iter()
                    .all(|unit| matches!(unit, 9 | 10 | 13 | 32))
                {
                    return Ok(Text(value));
                }
                return Err(invalid());
            }
            92 => {
                let escaped = *units.get(index).ok_or_else(invalid)?;
                index += 1;
                let decoded = match escaped {
                    34 | 47 | 92 => escaped,
                    98 => 8,
                    102 => 12,
                    110 => 10,
                    114 => 13,
                    116 => 9,
                    117 => {
                        let digits = units.get(index..index + 4).ok_or_else(invalid)?;
                        let mut decoded = 0;
                        for &digit in digits {
                            let digit = char::from_u32(digit as u32)
                                .and_then(|character| character.to_digit(16))
                                .ok_or_else(invalid)?;
                            decoded = (decoded << 4) | digit as u16;
                        }
                        index += 4;
                        decoded
                    }
                    _ => return Err(invalid()),
                };
                value.push(decoded);
            }
            0..=31 => return Err(invalid()),
            _ => value.push(unit),
        }
    }
    Err(invalid())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_surrogates_and_javascript_whitespace() -> Result<(), Error> {
        let value = parse_json_string(&Text::from("\"\\ud800\\ud83d\\ude00\\udfff\"").0)?;
        assert_eq!(value.json(), "\"\\ud800😀\\udfff\"");
        assert_eq!(Text(vec![0xd800]).utf8_len(), 3);
        assert_eq!(Text(vec![0xd83d, 0xde00]).utf8_len(), 4);
        assert!(is_space(0xfeff));
        assert!(!is_space(0x0085));
        assert_eq!(trim(&[0xfeff, 65, 0x2028]), &[65]);
        Ok(())
    }

    #[test]
    fn rejects_malformed_json_strings() {
        for text in ["", "\"", "\"\\x41\"", "\"\\u123z\"", "\"a\"b", "\"a\nb\""] {
            assert!(parse_json_string(&Text::from(text).0).is_err(), "{text:?}");
        }
    }
}
