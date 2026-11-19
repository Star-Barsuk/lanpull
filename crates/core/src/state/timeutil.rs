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
/// seconds.
///
/// Units are `s`, `m`, `h`, `d`; compound values add up. This is the whole
/// grammar the server needs: RUSTSEC-2026-0542 flags the humantime crate
/// unmaintained, and a five-unit parser is smaller than the advisory.
pub fn parse_duration_secs(value: &str) -> Result<i64> {
    const UNITS: &[(&str, i64)] = &[("s", 1), ("m", 60), ("h", 3_600), ("d", 86_400)];
    let mut total: i64 = 0;
    let mut rest = value.trim();
    let mut matched = false;
    while !rest.is_empty() {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            return Err(Error::Duration(format!(
                "{value}: {rest} is not a duration"
            )));
        }
        let number: i64 = digits
            .parse()
            .map_err(|_| Error::Duration(format!("{value}: number is too large")))?;
        // The prefix was collected from `rest` itself, so stripping it always
        // succeeds; the lint cannot see that, and indexing would panic on a
        // multibyte boundary it also cannot rule out.
        rest = rest.strip_prefix(digits.as_str()).unwrap_or("");
        let unit: String = rest.chars().take_while(char::is_ascii_alphabetic).collect();
        if unit.is_empty() {
            return Err(Error::Duration(format!(
                "{value}: unit missing after {number}"
            )));
        }
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
        rest = rest.strip_prefix(unit.as_str()).unwrap_or(rest);
        matched = true;
    }
    if !matched {
        return Err(Error::Duration(format!("{value}: empty")));
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    // Tests may unwrap and use bare asserts for brevity; production code may not.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects
    )]
    use super::*;

    #[test]
    fn parses_single_and_compound_units() {
        for (value, expected) in [
            ("15m", 900),
            ("30s", 30),
            ("1h", 3_600),
            ("7d", 604_800),
            ("1h30m", 5_400),
            ("1d2h3m4s", 93_784),
            (" 15m ", 900),
        ] {
            assert!(
                matches!(parse_duration_secs(value), Ok(total) if total == expected),
                "{value} should parse to {expected}"
            );
        }
    }

    #[test]
    fn rejects_malformed_durations() {
        for value in [
            "",
            "   ",
            "nope",
            "15",
            "15x",
            "m",
            "1h30",
            "99999999999999999999s",
        ] {
            assert!(
                parse_duration_secs(value).is_err(),
                "{value:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_duration_overflow() {
        // 2e14 days fits i64, but times the unit factor overflows it.
        assert!(parse_duration_secs("200000000000000d").is_err());
    }

    #[test]
    fn unix_seconds_round_trip_through_rfc3339() {
        let epoch = 1_700_000_000;
        let rendered = iso8601(epoch);
        assert_eq!(rendered, "2023-11-14T22:13:20Z");
        assert!(matches!(parse_iso8601(&rendered), Ok(secs) if secs == epoch));
    }

    #[test]
    fn now_unix_is_a_plausible_epoch() {
        assert!(now_unix() > 1_700_000_000, "the clock is past 2023");
    }
}
