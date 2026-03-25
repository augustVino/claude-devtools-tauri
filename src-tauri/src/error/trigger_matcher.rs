//! Trigger matcher -- pattern matching for notification triggers.
//!
//! Provides utilities for:
//! - Case-insensitive regex pattern matching with ReDoS protection
//! - Ignore pattern checking
//! - Extracting fields from tool_use blocks
//! - Getting content blocks from messages

use std::sync::LazyLock;

use moka::sync::Cache;
use regex::Regex;

use crate::types::messages::{ParsedMessage, ToolCall};
use crate::utils::regex_validation::create_safe_regex;

// =============================================================================
// Regex Cache
// =============================================================================

/// Maximum number of compiled regexes to cache.
const MAX_CACHE_SIZE: usize = 500;

/// Thread-safe LRU cache for compiled regex patterns.
/// Key: original pattern string. Value: compiled regex (None if invalid/dangerous).
static REGEX_CACHE: LazyLock<Cache<String, Option<Regex>>> = LazyLock::new(|| {
    Cache::builder()
        .max_capacity(MAX_CACHE_SIZE as u64)
        .build()
});

// =============================================================================
// Pattern Matching
// =============================================================================

/// Test content against pattern (case-insensitive regex).
/// Returns false for invalid or dangerous patterns.
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

/// Returns true if content matches ANY ignore pattern.
/// Returns false for empty or missing ignore patterns.
pub fn matches_ignore_patterns(content: &str, ignore_patterns: &[String]) -> bool {
    if ignore_patterns.is_empty() {
        return false;
    }

    for pattern in ignore_patterns {
        if matches_pattern(content, pattern) {
            return true;
        }
        // Invalid or dangerous patterns are skipped (matches_pattern returns false)
    }

    false
}

// =============================================================================
// Field Extraction
// =============================================================================

/// Extract a field value from a tool_use input.
/// `match_field` is optional -- if None, returns None.
/// Returns the value as a string, or JSON-serialized if it is not a string.
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

/// Get content blocks from a message as JSON array.
/// Returns empty array if content is not an array.
pub fn get_content_blocks(message: &ParsedMessage) -> Vec<serde_json::Value> {
    match &message.content {
        serde_json::Value::Array(blocks) => blocks.clone(),
        _ => vec![],
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    // -- helpers --

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
        // Call twice with same pattern to exercise cache path
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
        // Invalid patterns are silently skipped (same as Electron behavior)
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
