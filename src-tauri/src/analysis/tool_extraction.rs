//! Tool use/result extraction utilities.
//!
//! Builds maps from messages for error detection and trigger checking.
//! Ported from Electron ToolResultExtractor.ts.

use std::collections::HashMap;

use crate::types::domain::MessageType;
use crate::types::messages::ParsedMessage;

/// Information about a tool use invocation.
#[derive(Debug, Clone)]
pub struct ToolUseInfo {
    pub name: String,
    pub input: serde_json::Value,
}

/// Information about a tool result.
#[derive(Debug, Clone)]
pub struct ToolResultInfo {
    pub content: serde_json::Value,
    pub is_error: bool,
}

/// Extracted tool result for error message building.
#[derive(Debug, Clone)]
pub struct ExtractedToolResult {
    pub tool_use_id: String,
    pub is_error: bool,
    pub content: serde_json::Value,
    pub tool_name: Option<String>,
}

/// Build a map from tool_use ID to ToolUseInfo.
/// Iterates all messages, checking toolCalls array.
pub fn build_tool_use_map(messages: &[ParsedMessage]) -> HashMap<String, ToolUseInfo> {
    let mut map = HashMap::new();

    for message in messages {
        if message.message_type != MessageType::Assistant {
            continue;
        }

        for call in &message.tool_calls {
            map.insert(
                call.id.clone(),
                ToolUseInfo {
                    name: call.name.clone(),
                    input: call.input.clone(),
                },
            );
        }
    }

    map
}

/// Build a map from tool_use ID to ToolResultInfo.
/// Checks toolResults array.
pub fn build_tool_result_map(messages: &[ParsedMessage]) -> HashMap<String, ToolResultInfo> {
    let mut map = HashMap::new();

    for message in messages {
        for result in &message.tool_results {
            map.insert(
                result.tool_use_id.clone(),
                ToolResultInfo {
                    content: result.content.clone(),
                    is_error: result.is_error,
                },
            );
        }
    }

    map
}

/// Estimate token count for content.
/// Uses simple character-based estimation (~4 chars per token).
pub fn estimate_tokens(content: &serde_json::Value) -> usize {
    let text = match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            serde_json::to_string(content).unwrap_or_default()
        }
        _ => content.to_string(),
    };

    (text.len() / 4).max(1)
}

/// Extract tool results from a single message.
pub fn extract_tool_results(
    message: &ParsedMessage,
    find_tool_name_fn: &dyn Fn(&ParsedMessage, &str) -> Option<String>,
) -> Vec<ExtractedToolResult> {
    message
        .tool_results
        .iter()
        .map(|result| {
            let tool_name = find_tool_name_fn(message, &result.tool_use_id);
            ExtractedToolResult {
                tool_use_id: result.tool_use_id.clone(),
                is_error: result.is_error,
                content: result.content.clone(),
                tool_name,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::messages::{ToolCall, ToolResult};
    use serde_json::json;

    fn make_assistant_message(
        tool_calls: Vec<ToolCall>,
        tool_results: Vec<ToolResult>,
    ) -> ParsedMessage {
        ParsedMessage {
            uuid: "test-uuid".to_string(),
            parent_uuid: None,
            message_type: MessageType::Assistant,
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
            role: Some("assistant".to_string()),
            content: json!("test"),
            usage: None,
            model: None,
            cwd: None,
            git_branch: None,
            agent_id: None,
            is_sidechain: false,
            is_meta: false,
            user_type: None,
            tool_calls,
            tool_results,
            source_tool_use_id: None,
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            is_compact_summary: None,
            request_id: None,
        }
    }

    fn make_tool_call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            input: json!({"file_path": "/test/file.rs"}),
            is_task: false,
            task_description: None,
            task_subagent_type: None,
        }
    }

    fn make_tool_result(id: &str, content: serde_json::Value, is_error: bool) -> ToolResult {
        ToolResult {
            tool_use_id: id.to_string(),
            content,
            is_error,
        }
    }

    #[test]
    fn test_build_tool_use_map_empty() {
        let map = build_tool_use_map(&[]);
        assert!(map.is_empty());
    }

    #[test]
    fn test_build_tool_use_map_basic() {
        let msg = make_assistant_message(
            vec![make_tool_call("tc1", "Read"), make_tool_call("tc2", "Bash")],
            vec![],
        );
        let map = build_tool_use_map(&[msg]);
        assert_eq!(map.len(), 2);
        assert_eq!(map["tc1"].name, "Read");
        assert_eq!(map["tc2"].name, "Bash");
    }

    #[test]
    fn test_build_tool_result_map_empty() {
        let map = build_tool_result_map(&[]);
        assert!(map.is_empty());
    }

    #[test]
    fn test_build_tool_result_map_basic() {
        let msg = make_assistant_message(
            vec![],
            vec![
                make_tool_result("tr1", json!("file content"), false),
                make_tool_result("tr2", json!("error msg"), true),
            ],
        );
        let map = build_tool_result_map(&[msg]);
        assert_eq!(map.len(), 2);
        assert!(!map["tr1"].is_error);
        assert!(map["tr2"].is_error);
    }

    #[test]
    fn test_estimate_tokens_string() {
        let tokens = estimate_tokens(&json!("hello world this is a test"));
        assert!(tokens > 0);
    }

    #[test]
    fn test_estimate_tokens_empty() {
        let tokens = estimate_tokens(&json!(""));
        assert_eq!(tokens, 1); // minimum is 1
    }

    #[test]
    fn test_estimate_tokens_array() {
        let tokens = estimate_tokens(&json!([{"type": "text", "text": "hello"}]));
        assert!(tokens > 0);
    }
}
