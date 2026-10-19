use std::time::{SystemTime, UNIX_EPOCH};

use time::OffsetDateTime;
use time::macros::format_description;

use crate::Error;

pub fn unix_milliseconds(time: SystemTime) -> Result<i64, Error> {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_millis()),
        Err(error) => i64::try_from(error.duration().as_millis()).map(|value| -value),
    }
    .map_err(|_| {
        Error::new(
            "RangeError",
            "System time is outside the supported millisecond range",
        )
    })
}

pub fn now_milliseconds() -> Result<i64, Error> {
    unix_milliseconds(SystemTime::now())
}

pub fn iso_timestamp(milliseconds: i64) -> Result<String, Error> {
    let time = OffsetDateTime::from_unix_timestamp_nanos(milliseconds as i128 * 1_000_000)
        .map_err(|_| {
            Error::new(
                "RangeError",
                "Timestamp is outside the supported UTC date range",
            )
        })?;
    time.format(format_description!(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
    ))
    .map_err(|error| Error::new("Error", format!("UTC timestamp formatting failed: {error}")))
}

pub fn now_iso() -> Result<String, Error> {
    iso_timestamp(now_milliseconds()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_wire_timestamps_have_exact_millisecond_precision() -> Result<(), Error> {
        assert_eq!(iso_timestamp(0)?, "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_timestamp(1)?, "1970-01-01T00:00:00.001Z");
        assert_eq!(iso_timestamp(-1)?, "1969-12-31T23:59:59.999Z");
        assert_eq!(iso_timestamp(951_782_400_123)?, "2000-02-29T00:00:00.123Z");
        assert!(iso_timestamp(i64::MAX).is_err());
        Ok(())
    }
}
