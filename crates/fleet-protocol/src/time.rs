//! The one ISO-8601 / RFC-3339 time seam (2026-06 audit T2.1).
//!
//! Fleet used to carry three separate hand-rolled parsers (in `fleet-host-core`,
//! `fleet-hub`, and `fleet-reporter`) that all silently mis-handled numeric UTC
//! offsets — `2026-01-01T00:00:00+00:00` failed the naive `Z`-only byte checks and
//! parsed as "unparseable", which in the sort path ranked such a session as age 0.
//! This module replaces all three with `jiff`, the current well-maintained datetime
//! library, so offsets, fractional seconds, and `Z` are all handled correctly and
//! in one place.

use jiff::Timestamp;

/// The current UTC time formatted as `YYYY-MM-DDTHH:MM:SSZ` (whole-second
/// precision, `Z` suffix) — the wire format Fleet timestamps have always used.
pub fn now_iso8601() -> String {
    Timestamp::now().strftime("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Parse an ISO-8601 / RFC-3339 timestamp to whole seconds since the Unix epoch.
///
/// Handles `Z`, numeric offsets (`+00:00`, `-05:00`, …), and fractional seconds
/// (truncated toward the epoch second). A naive datetime with no offset is
/// interpreted as UTC, matching the behavior of the hand-rolled parsers this
/// replaces. Returns `None` for anything jiff can't parse as an instant.
pub fn parse_epoch_secs(s: &str) -> Option<i64> {
    // Absolute forms: `…Z` and `…±HH:MM`.
    if let Ok(ts) = s.parse::<Timestamp>() {
        return Some(ts.as_second());
    }
    // Fallback: a naive civil datetime (no offset) is read as UTC.
    if let Ok(dt) = s.parse::<jiff::civil::DateTime>() {
        if let Ok(zoned) = dt.to_zoned(jiff::tz::TimeZone::UTC) {
            return Some(zoned.timestamp().as_second());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn z_offset_and_naive_all_agree_on_the_same_instant() {
        // The T2.1 bug: the old parsers accepted only `Z` and returned None for a
        // numeric offset. jiff must treat `Z`, `+00:00`, and the equivalent
        // shifted offset as the SAME instant, and a naive stamp as UTC.
        let z = parse_epoch_secs("2026-01-01T00:00:00Z").unwrap();
        assert_eq!(parse_epoch_secs("2026-01-01T00:00:00+00:00"), Some(z));
        assert_eq!(parse_epoch_secs("2026-01-01T00:00:00"), Some(z));
        // 05:00 at +05:00 is the same instant as 00:00 UTC.
        assert_eq!(parse_epoch_secs("2026-01-01T05:00:00+05:00"), Some(z));
    }

    #[test]
    fn fractional_seconds_truncate_to_the_whole_second() {
        let base = parse_epoch_secs("2026-01-01T00:00:00Z").unwrap();
        assert_eq!(parse_epoch_secs("2026-01-01T00:00:00.999Z"), Some(base));
    }

    #[test]
    fn unparseable_input_is_none() {
        assert_eq!(parse_epoch_secs(""), None);
        assert_eq!(parse_epoch_secs("not a timestamp"), None);
        assert_eq!(parse_epoch_secs("2026-13-40T99:99:99Z"), None);
    }

    #[test]
    fn now_is_well_formed_and_round_trips() {
        let s = now_iso8601();
        assert_eq!(s.len(), 20, "YYYY-MM-DDTHH:MM:SSZ is 20 chars: {s}");
        assert!(s.ends_with('Z'), "UTC 'Z' suffix: {s}");
        assert!(s.as_bytes()[10] == b'T', "date/time separator: {s}");
        // It must parse back to an instant within a small window of "now".
        let parsed = parse_epoch_secs(&s).expect("now_iso8601 must be parseable");
        let now = Timestamp::now().as_second();
        assert!((now - parsed).abs() <= 2, "round-trips to ~now: {s}");
    }
}
