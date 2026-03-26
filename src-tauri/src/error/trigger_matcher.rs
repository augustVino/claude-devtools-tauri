//! 触发器模式匹配工具。
//!
//! 提供以下功能：
//! - 大小写不敏感的正则模式匹配（含 ReDoS 防护）
//! - 忽略模式检查
//! - 从 tool_use 块中提取字段
//! - 获取消息的内容块

use std::sync::LazyLock;

use moka::sync::Cache;
use regex::Regex;

use crate::types::messages::{ParsedMessage, ToolCall};
use crate::utils::regex_validation::create_safe_regex;

// =============================================================================
// 正则缓存
// =============================================================================

/// 编译后正则表达式的最大缓存数量。
const MAX_CACHE_SIZE: usize = 500;

/// 线程安全的 LRU 缓存，用于存储已编译的正则表达式。
/// 键：原始模式字符串。值：编译后的正则（无效或危险的则为 None）。
static REGEX_CACHE: LazyLock<Cache<String, Option<Regex>>> = LazyLock::new(|| {
    Cache::builder()
        .max_capacity(MAX_CACHE_SIZE as u64)
        .build()
});

// =============================================================================
// 模式匹配
// =============================================================================

/// 测试内容是否匹配模式（大小写不敏感正则）。
/// 对无效或危险的 pattern 返回 false。
pub fn matches_pattern(content: &str, pattern: &str) -> bool {
    if let Some(cached) = REGEX_CACHE.get(pattern) {
        return match cached {
            Some(re) => re.is_match(content),
            None => false,
        };
    }

    let case_insensitive_pattern = format!("(?i){}", pattern);
    let re = create_safe_regex(&case_insensitive_pattern);
    REGEX_CACHE.insert(pattern.to_string(), re.clone());
    re.map(|r| r.is_match(content)).unwrap_or(false)
}

/// 当内容匹配任意忽略模式时返回 true。
/// 忽略模式列表为空时返回 false。
pub fn matches_ignore_patterns(content: &str, ignore_patterns: &[String]) -> bool {
    if ignore_patterns.is_empty() {
        return false;
    }

    for pattern in ignore_patterns {
        if matches_pattern(content, pattern) {
            return true;
        }
        // 无效或危险的 pattern 被跳过（matches_pattern 返回 false）
    }

    false
}

// =============================================================================
// 字段提取
// =============================================================================

/// 从 tool_use 的 input 中提取指定字段的值。
/// `match_field` 为可选参数 -- 若为 None 则返回 None。
/// 返回字符串值；若原始值不是字符串，则返回其 JSON 序列化结果。
pub fn extract_tool_use_field(tool_use: &ToolCall, match_field: Option<&str>) -> Option<String> {
    let field = match_field?;
    let value = tool_use.input.get(field)?;

    if value.is_string() {
        value.as_str().map(String::from)
    } else if !value.is_null() {
        Some(value.to_string())
    } else {
        None
    }
}

/// 获取消息的内容块（JSON 数组形式）。
/// 若 content 不是数组，则返回空数组。
pub fn get_content_blocks(message: &ParsedMessage) -> Vec<serde_json::Value> {
    match &message.content {
        serde_json::Value::Array(blocks) => blocks.clone(),
        _ => vec![],
    }
}

// =============================================================================
// 测试
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    // -- 辅助函数 --

    fn make_tool_call(input: Value) -> ToolCall {
        ToolCall {
            id: "tool-1".to_string(),
            name: "Read".to_string(),
            input,
            is_task: false,
            task_description: None,
            task_subagent_type: None,
        }
    }

    // -- matches_pattern --

    #[test]
    fn test_matches_pattern_simple() {
        assert!(matches_pattern("Hello World", "hello"));
        assert!(matches_pattern("Hello World", "world"));
        assert!(!matches_pattern("Hello World", "goodbye"));
    }

    #[test]
    fn test_matches_pattern_regex() {
        assert!(matches_pattern("error 42 found", r"error\s+\d+"));
        assert!(matches_pattern("ERROR 42 found", r"error\s+\d+"));
        assert!(!matches_pattern("no match here", r"error\s+\d+"));
    }

    #[test]
    fn test_matches_pattern_invalid() {
        assert!(!matches_pattern("any content", r"(?P<unclosed"));
        assert!(!matches_pattern("any content", "[invalid"));
    }

    #[test]
    fn test_matches_pattern_empty() {
        assert!(matches_pattern("", ""));
        assert!(matches_pattern("hello", ""));
    }

    #[test]
    fn test_matches_pattern_cache_hit() {
        // 使用相同 pattern 调用两次以触发缓存路径
        assert!(matches_pattern("hello", "hel+"));
        assert!(matches_pattern("hello", "hel+"));
    }

    // -- matches_ignore_patterns --

    #[test]
    fn test_matches_ignore_patterns_hit() {
        let patterns = vec!["ignore.*me".to_string(), "skip.*this".to_string()];
        assert!(matches_ignore_patterns("please ignore me now", &patterns));
        assert!(matches_ignore_patterns("skip this one", &patterns));
    }

    #[test]
    fn test_matches_ignore_patterns_miss() {
        let patterns = vec!["ignore.*me".to_string()];
        assert!(!matches_ignore_patterns("important content", &patterns));
    }

    #[test]
    fn test_matches_ignore_patterns_empty() {
        assert!(!matches_ignore_patterns("any content", &[]));
    }

    #[test]
    fn test_matches_ignore_patterns_skips_invalid() {
        // 无效的 pattern 被静默跳过（与 Electron 行为一致）
        let patterns = vec![r"(?P<bad".to_string(), "valid".to_string()];
        assert!(matches_ignore_patterns("valid match", &patterns));
    }

    // -- extract_tool_use_field --

    #[test]
    fn test_extract_tool_use_field_string() {
        let tool_call = make_tool_call(json!({"path": "/src/main.ts"}));
        assert_eq!(
            extract_tool_use_field(&tool_call, Some("path")),
            Some("/src/main.ts".to_string())
        );
    }

    #[test]
    fn test_extract_tool_use_field_non_string() {
        let tool_call = make_tool_call(json!({"offset": 42}));
        assert_eq!(
            extract_tool_use_field(&tool_call, Some("offset")),
            Some("42".to_string())
        );
    }

    #[test]
    fn test_extract_tool_use_field_missing() {
        let tool_call = make_tool_call(json!({"path": "/src/main.ts"}));
        assert_eq!(extract_tool_use_field(&tool_call, Some("missing")), None);
    }

    #[test]
    fn test_extract_tool_use_field_none() {
        let tool_call = make_tool_call(json!({"path": "/src/main.ts"}));
        assert_eq!(extract_tool_use_field(&tool_call, None), None);
    }

    #[test]
    fn test_extract_tool_use_field_empty_input() {
        let tool_call = make_tool_call(json!({}));
        assert_eq!(extract_tool_use_field(&tool_call, Some("any")), None);
    }

    // -- get_content_blocks --

    #[test]
    fn test_get_content_blocks_array() {
        let message = ParsedMessage {
            uuid: "u1".to_string(),
            parent_uuid: None,
            message_type: crate::types::domain::MessageType::User,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            role: Some("user".to_string()),
            content: json!([{"type": "text", "text": "hello"}]),
            usage: None,
            model: None,
            cwd: None,
            git_branch: None,
            agent_id: None,
            is_sidechain: false,
            is_meta: false,
            user_type: None,
            tool_calls: vec![],
            tool_results: vec![],
            source_tool_use_id: None,
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            is_compact_summary: None,
            request_id: None,
        };
        let blocks = get_content_blocks(&message);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "text");
    }

    #[test]
    fn test_get_content_blocks_string() {
        let message = ParsedMessage {
            uuid: "u2".to_string(),
            parent_uuid: None,
            message_type: crate::types::domain::MessageType::User,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            role: Some("user".to_string()),
            content: json!("plain text content"),
            usage: None,
            model: None,
            cwd: None,
            git_branch: None,
            agent_id: None,
            is_sidechain: false,
            is_meta: false,
            user_type: None,
            tool_calls: vec![],
            tool_results: vec![],
            source_tool_use_id: None,
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            is_compact_summary: None,
            request_id: None,
        };
        let blocks = get_content_blocks(&message);
        assert!(blocks.is_empty());
    }

    #[test]
    fn test_get_content_blocks_empty_array() {
        let message = ParsedMessage {
            uuid: "u3".to_string(),
            parent_uuid: None,
            message_type: crate::types::domain::MessageType::User,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            role: Some("user".to_string()),
            content: json!([]),
            usage: None,
            model: None,
            cwd: None,
            git_branch: None,
            agent_id: None,
            is_sidechain: false,
            is_meta: false,
            user_type: None,
            tool_calls: vec![],
            tool_results: vec![],
            source_tool_use_id: None,
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            is_compact_summary: None,
            request_id: None,
        };
        let blocks = get_content_blocks(&message);
        assert!(blocks.is_empty());
    }
}
