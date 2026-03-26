//! 正则表达式安全创建与验证模块，提供 ReDoS（正则拒绝服务）防护。

use regex::Regex;
use std::time::Instant;

/// 正则编译最大允许耗时（毫秒），防止 ReDoS 攻击。
const REGEX_COMPILE_TIMEOUT_MS: u64 = 500;

/// 正则表达式模式的最大长度。
const MAX_PATTERN_LENGTH: usize = 10_000;

/// 正则模式被拒绝时返回的错误信息。
#[derive(Debug, Clone)]
pub struct RegexValidationError {
    pub pattern: String,
    pub reason: String,
}

/// 正则验证结果。
#[derive(Debug, Clone)]
pub struct RegexValidationResult {
    pub valid: bool,
    pub error: Option<RegexValidationError>,
}

/// 安全地创建正则表达式。若模式危险或无效则返回 `None`。
///
/// 通过限制编译时间和模式长度来防御 ReDoS 攻击。
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

/// 验证正则模式的有效性（轻量检查，不创建安全正则）。
/// 由 TriggerManager 用于触发器验证。
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
