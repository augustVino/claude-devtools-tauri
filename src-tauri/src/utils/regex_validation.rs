//! 正则表达式安全创建与验证模块，提供 ReDoS（正则拒绝服务）防护。
//!
//! 从 Electron `regexValidation.ts` 移植而来，包括危险模式检测和括号平衡检查。

use regex::Regex;
use std::time::Instant;

/// 正则编译最大允许耗时（毫秒），防止 ReDoS 攻击。
const REGEX_COMPILE_TIMEOUT_MS: u64 = 500;

/// 正则表达式模式的最大长度。
const MAX_PATTERN_LENGTH: usize = 100;

/// 编译后的危险模式集合，用于 ReDoS 静态检测。
///
/// 这些模式匹配已知的可导致指数级回溯的正则构造：
/// - 嵌套量词：`(a+)+`、`(a*)*`
/// - 重叠交替加量词：`(a|a)+`
/// - 重复量词：`a{1,}+`、`a{2}+`
/// - 反向引用加量词：`\1+`
/// - 长字符类加量词：`[...20+ chars]+`
static DANGEROUS_PATTERNS: std::sync::LazyLock<Vec<Regex>> = std::sync::LazyLock::new(|| {
    [
        r"\([^)]{0,50}[+*][^)]{0,50}\)[+*]",
        r"\([^)|]{0,50}\|[^)]{0,50}\)[+*]",
        r"[+*]\{",
        r"\}[+*]",
        r"\\[1-9][+*]",
        r"\[[^\]]{20,}\][+*]",
    ]
    .iter()
    .filter_map(|p| Regex::new(p).ok())
    .collect()
});

/// 正则模式被拒绝时返回的错误信息。
#[derive(Debug, Clone)]
#[allow(dead_code)]
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

/// 检查模式是否包含可能导致 ReDoS 的危险构造。
fn has_dangerous_patterns(pattern: &str) -> bool {
    DANGEROUS_PATTERNS.iter().any(|re| re.is_match(pattern))
}

/// 检查模式中的括号是否平衡。
///
/// 跟踪转义序列、字符类状态，匹配 `()`、`[]`、`{}` 对。
/// 与 Electron 端 `areBracketsBalanced()` 对齐。
fn are_brackets_balanced(pattern: &str) -> bool {
    let mut stack: Vec<char> = Vec::new();
    let close_to_open: [(char, char); 3] = [(')', '('), (']', '['), ('}', '{')];
    let mut escaped = false;
    let mut in_char_class = false;

    for ch in pattern.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }

        if ch == '[' && !in_char_class {
            in_char_class = true;
            stack.push(ch);
            continue;
        }
        if ch == ']' && in_char_class {
            in_char_class = false;
            if stack.last() != Some(&'[') {
                return false;
            }
            stack.pop();
            continue;
        }
        if in_char_class {
            continue;
        }

        if ch == '(' || ch == '{' {
            stack.push(ch);
        } else if let Some(expected_open) = close_to_open.iter().find(|(c, _)| *c == ch).map(|(_, o)| *o) {
            if stack.last() != Some(&expected_open) {
                return false;
            }
            stack.pop();
        }
    }

    stack.is_empty()
}

/// 安全地创建正则表达式。若模式危险或无效则返回 `None`。
///
/// 通过静态危险模式检测、括号平衡检查、限制编译时间和模式长度来防御 ReDoS 攻击。
pub fn create_safe_regex(pattern: &str) -> Option<Regex> {
    if pattern.is_empty() || pattern.len() > MAX_PATTERN_LENGTH {
        return None;
    }
    if has_dangerous_patterns(pattern) || !are_brackets_balanced(pattern) {
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

/// 验证正则模式的有效性。
///
/// 检查流程（与 Electron 端 `validateRegexPattern` 对齐）：
/// 1. 空模式检查
/// 2. 长度限制（最大 100 字符）
/// 3. 危险模式检测（嵌套量词、重叠交替、反向引用等）
/// 4. 括号平衡检查
/// 5. 正则语法检查（编译测试）
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

    if has_dangerous_patterns(pattern) {
        return RegexValidationResult {
            valid: false,
            error: Some(RegexValidationError {
                pattern: pattern.to_string(),
                reason: "Pattern contains constructs that could cause performance issues"
                    .to_string(),
            }),
        };
    }

    if !are_brackets_balanced(pattern) {
        return RegexValidationResult {
            valid: false,
            error: Some(RegexValidationError {
                pattern: pattern.to_string(),
                reason: "Pattern has unbalanced brackets".to_string(),
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

    // --- New tests for dangerous pattern detection ---

    #[test]
    fn test_nested_quantifiers_rejected() {
        let result = validate_regex_pattern("(a+)+");
        assert!(!result.valid);
        assert!(result.error.as_ref().unwrap().reason.contains("performance"));
    }

    #[test]
    fn test_overlapping_alternation_rejected() {
        let result = validate_regex_pattern("(a|a)+");
        assert!(!result.valid);
    }

    #[test]
    fn test_backreference_with_quantifier_rejected() {
        let result = validate_regex_pattern(r"(a+)\1+");
        assert!(!result.valid);
    }

    #[test]
    fn test_long_char_class_with_quantifier_rejected() {
        let result = validate_regex_pattern("[abcdefghijklmnopqrstuvwxyz]+");
        assert!(!result.valid);
    }

    #[test]
    fn test_safe_pattern_accepted() {
        assert!(validate_regex_pattern(r"error|warning").valid);
        assert!(validate_regex_pattern(r"\[ERROR\]").valid);
        assert!(validate_regex_pattern(r"timeout \d+s").valid);
    }

    // --- New tests for bracket balance ---

    #[test]
    fn test_unbalanced_parentheses_rejected() {
        let result = validate_regex_pattern("(abc");
        assert!(!result.valid);
        assert!(result.error.as_ref().unwrap().reason.contains("unbalanced"));
    }

    #[test]
    fn test_unbalanced_brackets_rejected() {
        let result = validate_regex_pattern("[abc");
        assert!(!result.valid);
        assert!(result.error.as_ref().unwrap().reason.contains("unbalanced"));
    }

    #[test]
    fn test_balanced_brackets_accepted() {
        assert!(validate_regex_pattern("(a(b)c)").valid);
        assert!(validate_regex_pattern("[a-z]{1,3}").valid);
    }
}
