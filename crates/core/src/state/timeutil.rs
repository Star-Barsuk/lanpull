//! Time formatting and duration parsing helpers.

use std::time::{SystemTime, UNIX_EPOCH};

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

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

/// Parse a human duration such as `15m`, `30m`, `1h30m`, or `7d` into
/// seconds. Units are `s`, `m`, `h`, `d`; compound values add up. This is
/// the whole grammar the server needs: RUSTSEC-2026-0542 flags the
/// humantime crate unmaintained, and a five-unit parser is smaller than
/// the advisory.
pub fn parse_duration_secs(value: &str) -> Result<i64> {
    const UNITS: &[(&str, i64)] = &[("s", 1), ("m", 60), ("h", 3_600), ("d", 86_400)];
    let mut total: i64 = 0;
    let mut rest = value.trim();
    let mut matched = false;
    while !rest.is_empty() {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 {
            return Err(Error::Duration(format!("{value}: {rest} is not a duration")));
        }
        let number: i64 = rest[..digits]
            .parse()
            .map_err(|_| Error::Duration(format!("{value}: number is too large")))?;
        rest = &rest[digits..];
        let unit_len = rest.chars().take_while(char::is_ascii_alphabetic).count();
        if unit_len == 0 {
            return Err(Error::Duration(format!(
                "{value}: unit missing after {}",
                rest
            )));
        }
        let unit = &rest[..unit_len];
        let factor = UNITS
            .iter()
            .find(|(name, _)| *name == unit)
            .map(|(_, seconds)| seconds)
            .ok_or_else(|| Error::Duration(format!("{value}: unknown unit {unit}")))?;
        let term = number
            .checked_mul(*factor)
            .ok_or_else(|| Error::Duration(format!("{value}: duration is too large")))?;
        total = total
            .checked_add(term)
            .ok_or_else(|| Error::Duration(format!("{value}: duration is too large")))?;
        rest = &rest[unit_len..];
        matched = true;
    }
    if !matched {
        return Err(Error::Duration(format!("{value}: empty")));
    }
    Ok(total)
}
