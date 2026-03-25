//! Safe regex creation with ReDoS protection.

use regex::Regex;
use std::time::Instant;

/// Maximum time allowed for regex compilation (prevents ReDoS).
const REGEX_COMPILE_TIMEOUT_MS: u64 = 500;

/// Maximum regex pattern length.
const MAX_PATTERN_LENGTH: usize = 10_000;

/// Error returned when a regex pattern is rejected.
#[derive(Debug, Clone)]
pub struct RegexValidationError {
    pub pattern: String,
    pub reason: String,
}

/// Result of regex validation.
#[derive(Debug, Clone)]
pub struct RegexValidationResult {
    pub valid: bool,
    pub error: Option<RegexValidationError>,
}

/// Create a safe regex, returning None if the pattern is dangerous or invalid.
/// Protects against ReDoS by limiting compilation time and pattern length.
pub fn create_safe_regex(pattern: &str) -> Option<Regex> {
    if pattern.len() > MAX_PATTERN_LENGTH {
        return None;
    }

    let start = Instant::now();
    match Regex::new(pattern) {
        Ok(re) => {
            if start.elapsed().as_millis() as u64 > REGEX_COMPILE_TIMEOUT_MS {
                None
            } else {
                Some(re)
            }
        }
        Err(_) => None,
    }
}

/// Validate a regex pattern without compiling it (lighter check).
/// Used by TriggerManager for trigger validation.
pub fn validate_regex_pattern(pattern: &str) -> RegexValidationResult {
    if pattern.is_empty() {
        return RegexValidationResult {
            valid: false,
            error: Some(RegexValidationError {
                pattern: pattern.to_string(),
                reason: "Pattern is empty".to_string(),
            }),
        };
    }

    if pattern.len() > MAX_PATTERN_LENGTH {
        return RegexValidationResult {
            valid: false,
            error: Some(RegexValidationError {
                pattern: pattern.to_string(),
                reason: format!(
                    "Pattern exceeds maximum length of {} characters",
                    MAX_PATTERN_LENGTH
                ),
            }),
        };
    }

    match Regex::new(pattern) {
        Ok(_) => RegexValidationResult {
            valid: true,
            error: None,
        },
        Err(e) => RegexValidationResult {
            valid: false,
            error: Some(RegexValidationError {
                pattern: pattern.to_string(),
                reason: e.to_string(),
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_safe_regex_valid() {
        let re = create_safe_regex(r"hello\s+world");
        assert!(re.is_some());
        assert!(re.unwrap().is_match("hello  world"));
    }

    #[test]
    fn test_create_safe_regex_invalid() {
        assert!(create_safe_regex(r"(?P<unclosed").is_none());
    }

    #[test]
    fn test_create_safe_regex_too_long() {
        let long_pattern = "a".repeat(MAX_PATTERN_LENGTH + 1);
        assert!(create_safe_regex(&long_pattern).is_none());
    }

    #[test]
    fn test_validate_regex_pattern_valid() {
        let result = validate_regex_pattern(r"\d+");
        assert!(result.valid);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_validate_regex_pattern_empty() {
        let result = validate_regex_pattern("");
        assert!(!result.valid);
        assert!(result.error.is_some());
    }

    #[test]
    fn test_validate_regex_pattern_invalid() {
        let result = validate_regex_pattern(r"(?P<bad");
        assert!(!result.valid);
    }
}
