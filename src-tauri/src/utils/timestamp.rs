//! Timestamp parsing utilities.
//!
//! Provides a single source of truth for converting RFC 3339 / ISO 8601
//! timestamp strings to epoch milliseconds.

/// Parse an RFC 3339 timestamp string to epoch milliseconds (`u64`).
///
/// Returns `0` if the timestamp cannot be parsed.
pub fn parse_ts_ms(ts: &str) -> u64 {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|dt| dt.timestamp_millis() as u64)
        .unwrap_or(0)
}

/// Parse an RFC 3339 timestamp string to epoch milliseconds (`u64`).
///
/// Returns `None` if the timestamp cannot be parsed.
pub fn parse_ts_ms_opt(ts: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.timestamp_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ts_ms_valid() {
        let ms = parse_ts_ms("2026-01-15T10:30:00.123Z");
        assert_eq!(ms, 1768473000123);
    }

    #[test]
    fn parse_ts_ms_invalid() {
        assert_eq!(parse_ts_ms("not-a-timestamp"), 0);
    }

    #[test]
    fn parse_ts_ms_empty() {
        assert_eq!(parse_ts_ms(""), 0);
    }

    #[test]
    fn parse_ts_ms_opt_valid() {
        assert!(parse_ts_ms_opt("2026-01-15T10:30:00Z").is_some());
    }

    #[test]
    fn parse_ts_ms_opt_invalid() {
        assert_eq!(parse_ts_ms_opt("not-a-timestamp"), None);
    }

    #[test]
    fn parse_ts_ms_opt_empty() {
        assert_eq!(parse_ts_ms_opt(""), None);
    }
}
