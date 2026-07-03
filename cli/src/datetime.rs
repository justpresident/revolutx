//! Human-friendly date/time parsing for CLI flags.
//!
//! Accepts a calendar date (`2024-01-31`), a month (`2024-01`), a year
//! (`2024`), a date and time (`"2024-01-31 14:30"` or `2024-01-31T14:30:00`), a
//! full RFC 3339 timestamp, a relative offset before now (`7d`, `24h`, `30m`,
//! `45s`, `2w`, or `now`), or a raw epoch integer. A bare integer in
//! `1970..=9999` is a year (a raw epoch that small would be an instant in early
//! 1970 — never what a query means); larger integers are auto-detected as epoch
//! seconds or milliseconds by magnitude (see [`epoch_millis`]), so a pasted
//! seconds value is not silently read as 1970. Naive forms are interpreted as
//! UTC. Returns Unix epoch milliseconds — what the API expects — so it plugs in
//! as a clap `value_parser` and the rest of the code keeps working with `i64`.

use std::fmt;

use time::format_description::well_known::Rfc3339;
use time::{Date, Duration, OffsetDateTime, PrimitiveDateTime};

/// Error from [`parse_when`]. A plain message that satisfies clap's value-parser
/// error bound.
#[derive(Debug)]
pub struct DateParseError(String);

impl fmt::Display for DateParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DateParseError {}

/// Parses a human date/time (or epoch millis) to Unix epoch milliseconds.
pub fn parse_when(input: &str) -> Result<i64, DateParseError> {
    let s = input.trim();
    if let Some(millis) = parse_relative(s) {
        return Ok(millis);
    }
    if let Ok(odt) = OffsetDateTime::parse(s, &Rfc3339) {
        return Ok(millis_of(odt));
    }
    for fmt in [
        "[year]-[month]-[day]T[hour]:[minute]:[second]",
        "[year]-[month]-[day] [hour]:[minute]:[second]",
        "[year]-[month]-[day] [hour]:[minute]",
        "[year]-[month]-[day]",
    ] {
        if let Some(millis) = parse_fixed(s, fmt) {
            return Ok(millis);
        }
    }
    if let Some(millis) = parse_year_month(s) {
        return Ok(millis);
    }
    if let Ok(n) = s.parse::<i64>() {
        // A bare 1970..=9999 is a year: as an epoch it would be an instant in
        // the first minutes of 1970, which no query ever means — while `2026`
        // as "the year 2026" is exactly what a human types.
        if let Ok(year) = i32::try_from(n)
            && (1970..=9999).contains(&year)
        {
            return calendar_millis(year, 1, 1)
                .ok_or_else(|| DateParseError(format!("invalid year '{input}'")));
        }
        return Ok(epoch_millis(n));
    }
    Err(DateParseError(format!(
        "invalid date/time '{input}' — use e.g. 2024-01-31, 2024-01, 2024, \
         \"2024-01-31 14:30\", an RFC3339 timestamp, a relative 7d/24h/30m, or epoch \
         seconds/milliseconds"
    )))
}

/// `2024-05` → midnight UTC on the first of that month.
fn parse_year_month(s: &str) -> Option<i64> {
    let (year, month) = s.split_once('-')?;
    if year.len() != 4 {
        return None;
    }
    let year: i32 = year.parse().ok()?;
    let month: u8 = month.parse().ok()?;
    calendar_millis(year, month, 1)
}

/// Midnight UTC on the given calendar day, as epoch milliseconds.
fn calendar_millis(year: i32, month: u8, day: u8) -> Option<i64> {
    let month = time::Month::try_from(month).ok()?;
    let date = Date::from_calendar_date(year, month, day).ok()?;
    Some(millis_of(date.with_hms(0, 0, 0).ok()?.assume_utc()))
}

/// Magnitude at or above which a bare epoch integer is read as milliseconds, and
/// below which as seconds (then scaled up). `1e11` ms is ~1973 and `1e11` s is
/// ~year 5138, so every realistic exchange timestamp is unambiguous either way —
/// this only reinterprets pre-1973 millisecond values, which no query uses.
const SECONDS_MS_BOUNDARY: i64 = 100_000_000_000;

/// Normalizes a raw epoch integer to milliseconds, treating small magnitudes as
/// seconds so a pasted seconds value is not read as an instant in early 1970.
const fn epoch_millis(n: i64) -> i64 {
    if n.abs() < SECONDS_MS_BOUNDARY {
        n * 1000
    } else {
        n
    }
}

const fn millis_of(odt: OffsetDateTime) -> i64 {
    // Add the millisecond component so sub-second precision (e.g. RFC3339 `.500`)
    // is preserved rather than truncated by a bare seconds×1000 conversion.
    odt.unix_timestamp() * 1000 + odt.millisecond() as i64
}

/// `7d`, `24h`, `30m`, `45s`, `2w`, or `now` → an instant at or before now.
fn parse_relative(s: &str) -> Option<i64> {
    if s.eq_ignore_ascii_case("now") {
        return Some(millis_of(OffsetDateTime::now_utc()));
    }
    let split = s.find(|c: char| c.is_ascii_alphabetic())?;
    let (count, unit) = s.split_at(split);
    let n: i64 = count.parse().ok()?;
    let duration = match unit {
        "s" => Duration::seconds(n),
        "m" => Duration::minutes(n),
        "h" => Duration::hours(n),
        "d" => Duration::days(n),
        "w" => Duration::weeks(n),
        _ => return None,
    };
    Some(millis_of(OffsetDateTime::now_utc() - duration))
}

/// Parses `s` against a fixed format — as a datetime, else a date at UTC midnight.
fn parse_fixed(s: &str, fmt: &str) -> Option<i64> {
    let desc = time::format_description::parse(fmt).ok()?;
    if let Ok(pdt) = PrimitiveDateTime::parse(s, &desc) {
        return Some(millis_of(pdt.assume_utc()));
    }
    let date = Date::parse(s, &desc).ok()?;
    Some(millis_of(date.with_hms(0, 0, 0).ok()?.assume_utc()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::parse_when;

    #[test]
    fn parses_a_date_as_utc_midnight() {
        // 2024-01-31T00:00:00Z = 1706659200 s.
        assert_eq!(parse_when("2024-01-31").unwrap(), 1_706_659_200_000);
    }

    #[test]
    fn parses_date_and_time() {
        assert_eq!(parse_when("2024-01-31 14:30").unwrap(), 1_706_711_400_000);
        assert_eq!(
            parse_when("2024-01-31T14:30:00").unwrap(),
            1_706_711_400_000
        );
    }

    #[test]
    fn parses_rfc3339_and_epoch_millis() {
        assert_eq!(
            parse_when("2024-01-31T14:30:00Z").unwrap(),
            1_706_711_400_000
        );
        assert_eq!(parse_when("1706659200000").unwrap(), 1_706_659_200_000);
    }

    #[test]
    fn bare_epoch_seconds_are_scaled_to_millis() {
        // A pasted seconds value is detected by magnitude and scaled, not read as
        // an instant 1000× too early.
        assert_eq!(parse_when("1706659200").unwrap(), 1_706_659_200_000);
    }

    #[test]
    fn rfc3339_subsecond_precision_is_preserved() {
        assert_eq!(
            parse_when("2024-01-31T14:30:00.500Z").unwrap(),
            1_706_711_400_500
        );
    }

    #[test]
    fn relative_is_before_now() {
        let now = time::OffsetDateTime::now_utc().unix_timestamp() * 1000;
        let day_ago = parse_when("1d").unwrap();
        let delta = now - day_ago;
        // ~24h ago, give or take the test's runtime.
        assert!((86_300_000..=86_500_000).contains(&delta), "delta={delta}");
    }

    #[test]
    fn bare_year_and_year_month_are_calendar_dates_not_epochs() {
        // `2026` used to be read as a bare epoch (an instant in early 1970),
        // which sent range queries walking half a century of empty windows.
        assert_eq!(parse_when("2026").unwrap(), 1_767_225_600_000); // 2026-01-01T00:00:00Z
        assert_eq!(parse_when("2026-05").unwrap(), 1_777_593_600_000); // 2026-05-01T00:00:00Z
        // Realistic epochs (too large to be years) still parse as epochs.
        assert_eq!(parse_when("1706659200").unwrap(), 1_706_659_200_000);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_when("not-a-date").is_err());
        assert!(parse_when("2026-13").is_err()); // no thirteenth month
    }
}
