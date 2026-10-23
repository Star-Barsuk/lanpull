//! Time formatting and duration parsing helpers.

use std::time::{SystemTime, UNIX_EPOCH};

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::error::{Error, Result};

/// Current wall-clock time as Unix seconds.
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

/// Format Unix seconds as an RFC 3339 UTC timestamp.
pub fn iso8601(epoch: i64) -> String {
    OffsetDateTime::from_unix_timestamp(epoch).map_or_else(
        |_| "1970-01-01T00:00:00Z".to_string(),
        |dt| {
            dt.format(&Rfc3339)
                .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
        },
    )
}

/// Parse an RFC 3339 timestamp into Unix seconds.
pub fn parse_iso8601(value: &str) -> Result<i64> {
    let parsed = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|e| Error::Manifest(format!("bad timestamp {value}: {e}")))?;
    Ok(parsed.unix_timestamp())
}

/// Parse a human duration such as `15m`, `30m`, or `7d` into seconds.
pub fn parse_duration_secs(value: &str) -> Result<i64> {
    let parsed =
        humantime::parse_duration(value).map_err(|e| Error::Duration(format!("{value}: {e}")))?;
    i64::try_from(parsed.as_secs())
        .map_err(|_| Error::Duration(format!("{value}: duration is too large")))
}
