//! 错误消息构建器 —— 从工具结果中提取错误文本并创建
//! [`DetectedError`] 实例。
//!
//! 从 Electron `src/main/services/error/ErrorMessageBuilder.ts` 移植而来。

use uuid::Uuid;

use crate::analysis::tool_extraction::ExtractedToolResult;
use crate::types::config::{DetectedError, ErrorContext};
use crate::types::messages::ParsedMessage;

// =============================================================================
// 常量
// =============================================================================

/// 错误消息截断前的最大长度。
const MAX_ERROR_MESSAGE_LENGTH: usize = 500;

// =============================================================================
// 类型定义
// =============================================================================

/// 创建 [`DetectedError`] 所需的参数。
#[derive(Debug, Clone)]
pub struct CreateDetectedErrorParams {
    pub session_id: String,
    pub project_id: String,
    pub file_path: String,
    pub project_name: String,
    pub line_number: u64,
    pub source: String,
    pub message: String,
    pub timestamp: u64,
    pub cwd: Option<String>,
    pub tool_use_id: Option<String>,
    pub subagent_id: Option<String>,
    pub trigger_color: Option<String>,
    pub trigger_id: Option<String>,
    pub trigger_name: Option<String>,
}

// =============================================================================
// 错误消息提取
// =============================================================================

/// 从工具结果中提取错误消息。
///
/// 支持字符串内容和数组内容块（`"type": "text"` 对象）两种形式。
/// 当无法提取文本时，回退为 `"Unknown error"`。

pub fn extract_error_message(result: &ExtractedToolResult) -> String {
    match &result.content {
        serde_json::Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                "Unknown error".to_string()
            } else {
                trimmed.to_string()
            }
        }
        serde_json::Value::Array(arr) => {
            let texts: Vec<&str> = arr
                .iter()
                .filter_map(|item| {
                    let obj = item.as_object()?;
                    if obj.get("type")?.as_str()? == "text" {
                        obj.get("text")?.as_str()
                    } else {
                        None
                    }
                })
                .collect();

            let joined = texts.join("\n").trim().to_string();
            if joined.is_empty() {
                "Unknown error".to_string()
            } else {
                joined
            }
        }
        _ => "Unknown error".to_string(),
    }
}

// =============================================================================
// 工具名称查找
// =============================================================================

/// 根据工具调用 ID 从消息的 tool_calls 中查找工具名称。
fn find_tool_name(message: &ParsedMessage, tool_use_id: &str) -> Option<String> {
    message
        .tool_calls
        .iter()
        .find(|tc| tc.id == tool_use_id)
        .map(|tc| tc.name.clone())
}

/// 在消息上下文中根据 tool_use_id 查找工具名称。
///
/// 首先检查 `toolCalls` 数组，然后当 `sourceToolUseID` 匹配时，
/// 回退到 `toolUseResult` 的 `toolName` 字段。
pub fn find_tool_name_by_tool_use_id(
    message: &ParsedMessage,
    tool_use_id: &str,
) -> Option<String> {
    // 优先从 toolCalls 中查找
    if let Some(name) = find_tool_name(message, tool_use_id) {
        return Some(name);
    }

    // 若此消息为工具结果，则检查 sourceToolUseID
    if let Some(ref source_id) = message.source_tool_use_id {
        if source_id == tool_use_id {
            if let Some(ref tool_result) = message.tool_use_result {
                if let Some(name) = tool_result.get("toolName").and_then(|v| v.as_str()) {
                    return Some(name.to_string());
                }
            }
        }
    }

    None
}

// =============================================================================
// 消息截断
// =============================================================================

/// 将错误消息截断到适合显示的长度。
fn truncate_message(message: &str) -> String {
    if message.len() <= MAX_ERROR_MESSAGE_LENGTH {
        return message.to_string();
    }
    format!("{}...", &message[..MAX_ERROR_MESSAGE_LENGTH])
}

// =============================================================================
// DetectedError 创建
// =============================================================================

/// 创建一个包含所有必需字段的 [`DetectedError`] 对象。
///
/// 为错误 ID 生成 UUID，并将消息截断到最大显示长度。

pub fn create_detected_error(params: CreateDetectedErrorParams) -> DetectedError {
    DetectedError {
        id: Uuid::new_v4().to_string(),
        session_id: params.session_id,
        project_id: params.project_id,
        file_path: params.file_path,
        source: params.source,
        message: truncate_message(&params.message),
        timestamp: params.timestamp,
        line_number: Some(params.line_number),
        tool_use_id: params.tool_use_id,
        subagent_id: params.subagent_id,
        trigger_color: params.trigger_color,
        trigger_id: params.trigger_id,
        trigger_name: params.trigger_name,
        context: ErrorContext {
            project_name: params.project_name,
            cwd: params.cwd,
        },
    }
}

// =============================================================================
// 测试
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::domain::MessageType;
    use crate::types::messages::{ToolCall, ToolResult};
    use serde_json::json;

    // ---------------------------------------------------------------------------
    // 辅助函数
    // ---------------------------------------------------------------------------

    fn make_extracted_tool_result(
        tool_use_id: &str,
        content: serde_json::Value,
        is_error: bool,
    ) -> ExtractedToolResult {
        ExtractedToolResult {
            tool_use_id: tool_use_id.to_string(),
            is_error,
            content,
            tool_name: None,
        }
    }

    fn make_parsed_message(
        tool_calls: Vec<ToolCall>,
        tool_results: Vec<ToolResult>,
        source_tool_use_id: Option<&str>,
        tool_use_result: Option<serde_json::Value>,
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
            source_tool_use_id: source_tool_use_id.map(String::from),
            source_tool_assistant_uuid: None,
            tool_use_result,
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

    // ---------------------------------------------------------------------------
    // extract_error_message tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_extract_error_message_string_content() {
        let result = make_extracted_tool_result("tu1", json!("Build failed: error E0425"), true);
        let msg = extract_error_message(&result);
        assert_eq!(msg, "Build failed: error E0425");
    }

    #[test]
    fn test_extract_error_message_string_whitespace_trimmed() {
        let result = make_extracted_tool_result("tu1", json!("  spaced error  "), true);
        let msg = extract_error_message(&result);
        assert_eq!(msg, "spaced error");
    }

    #[test]
    fn test_extract_error_message_empty_string() {
        let result = make_extracted_tool_result("tu1", json!(""), true);
        let msg = extract_error_message(&result);
        assert_eq!(msg, "Unknown error");
    }

    #[test]
    fn test_extract_error_message_whitespace_only_string() {
        let result = make_extracted_tool_result("tu1", json!("   "), true);
        let msg = extract_error_message(&result);
        assert_eq!(msg, "Unknown error");
    }

    #[test]
    fn test_extract_error_message_array_text_blocks() {
        let content = json!([
            {"type": "text", "text": "Line 1 error"},
            {"type": "text", "text": "Line 2 error"}
        ]);
        let result = make_extracted_tool_result("tu1", content, true);
        let msg = extract_error_message(&result);
        assert_eq!(msg, "Line 1 error\nLine 2 error");
    }

    #[test]
    fn test_extract_error_message_array_non_text_blocks_ignored() {
        let content = json!([
            {"type": "thinking", "thinking": "hmm"},
            {"type": "text", "text": "actual error"}
        ]);
        let result = make_extracted_tool_result("tu1", content, true);
        let msg = extract_error_message(&result);
        assert_eq!(msg, "actual error");
    }

    #[test]
    fn test_extract_error_message_array_empty() {
        let result = make_extracted_tool_result("tu1", json!([]), true);
        let msg = extract_error_message(&result);
        assert_eq!(msg, "Unknown error");
    }

    #[test]
    fn test_extract_error_message_array_no_text_blocks() {
        let content = json!([
            {"type": "image", "source": {"type": "base64"}}
        ]);
        let result = make_extracted_tool_result("tu1", content, true);
        let msg = extract_error_message(&result);
        assert_eq!(msg, "Unknown error");
    }

    #[test]
    fn test_extract_error_message_non_string_non_array() {
        let result = make_extracted_tool_result("tu1", json!(42), true);
        let msg = extract_error_message(&result);
        assert_eq!(msg, "Unknown error");
    }

    #[test]
    fn test_extract_error_message_null() {
        let result = make_extracted_tool_result("tu1", json!(null), true);
        let msg = extract_error_message(&result);
        assert_eq!(msg, "Unknown error");
    }

    #[test]
    fn test_extract_error_message_object_content() {
        let content = json!({"error": "something went wrong"});
        let result = make_extracted_tool_result("tu1", content, true);
        let msg = extract_error_message(&result);
        assert_eq!(msg, "Unknown error");
    }

    // ---------------------------------------------------------------------------
    // find_tool_name_by_tool_use_id tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_find_tool_name_from_tool_calls() {
        let msg = make_parsed_message(
            vec![
                make_tool_call("tu1", "Read"),
                make_tool_call("tu2", "Bash"),
            ],
            vec![],
            None,
            None,
        );
        assert_eq!(
            find_tool_name_by_tool_use_id(&msg, "tu1"),
            Some("Read".to_string())
        );
        assert_eq!(
            find_tool_name_by_tool_use_id(&msg, "tu2"),
            Some("Bash".to_string())
        );
    }

    #[test]
    fn test_find_tool_name_not_found() {
        let msg = make_parsed_message(vec![], vec![], None, None);
        assert_eq!(find_tool_name_by_tool_use_id(&msg, "tu_missing"), None);
    }

    #[test]
    fn test_find_tool_name_from_tool_use_result() {
        let msg = make_parsed_message(
            vec![],
            vec![],
            Some("tu1"),
            Some(json!({"toolName": "Write", "output": "ok"})),
        );
        assert_eq!(
            find_tool_name_by_tool_use_id(&msg, "tu1"),
            Some("Write".to_string())
        );
    }

    #[test]
    fn test_find_tool_name_tool_use_result_mismatched_id() {
        let msg = make_parsed_message(
            vec![],
            vec![],
            Some("tu_other"),
            Some(json!({"toolName": "Write"})),
        );
        // sourceToolUseID 为 "tu_other"，查询 "tu1" — 不应匹配
        assert_eq!(find_tool_name_by_tool_use_id(&msg, "tu1"), None);
    }

    #[test]
    fn test_find_tool_name_tool_calls_take_priority() {
        let msg = make_parsed_message(
            vec![make_tool_call("tu1", "Read")],
            vec![],
            Some("tu1"),
            Some(json!({"toolName": "Bash"})),
        );
        // toolCalls 匹配应优先于 toolUseResult
        assert_eq!(
            find_tool_name_by_tool_use_id(&msg, "tu1"),
            Some("Read".to_string())
        );
    }

    #[test]
    fn test_find_tool_name_tool_use_result_no_tool_name_field() {
        let msg = make_parsed_message(
            vec![],
            vec![],
            Some("tu1"),
            Some(json!({"output": "ok"})),
        );
        assert_eq!(find_tool_name_by_tool_use_id(&msg, "tu1"), None);
    }

    #[test]
    fn test_find_tool_name_tool_use_result_tool_name_not_string() {
        let msg = make_parsed_message(
            vec![],
            vec![],
            Some("tu1"),
            Some(json!({"toolName": 42})),
        );
        assert_eq!(find_tool_name_by_tool_use_id(&msg, "tu1"), None);
    }

    #[test]
    fn test_find_tool_name_tool_use_result_null() {
        let msg = make_parsed_message(
            vec![],
            vec![],
            Some("tu1"),
            None,
        );
        assert_eq!(find_tool_name_by_tool_use_id(&msg, "tu1"), None);
    }

    // ---------------------------------------------------------------------------
    // truncate_message tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_truncate_message_short() {
        assert_eq!(truncate_message("short"), "short");
    }

    #[test]
    fn test_truncate_message_exact_limit() {
        let msg = "a".repeat(MAX_ERROR_MESSAGE_LENGTH);
        assert_eq!(truncate_message(&msg), msg);
        assert!(!truncate_message(&msg).ends_with("..."));
    }

    #[test]
    fn test_truncate_message_over_limit() {
        let msg = "a".repeat(MAX_ERROR_MESSAGE_LENGTH + 100);
        let truncated = truncate_message(&msg);
        assert!(truncated.ends_with("..."));
        assert_eq!(truncated.len(), MAX_ERROR_MESSAGE_LENGTH + 3);
    }

    #[test]
    fn test_truncate_message_empty() {
        assert_eq!(truncate_message(""), "");
    }

    // ---------------------------------------------------------------------------
    // create_detected_error tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_create_detected_error_basic() {
        let params = CreateDetectedErrorParams {
            session_id: "session-123".to_string(),
            project_id: "proj-abc".to_string(),
            file_path: "/path/to/session.jsonl".to_string(),
            project_name: "my-project".to_string(),
            line_number: 42,
            source: "Bash".to_string(),
            message: "Build failed".to_string(),
            timestamp: 1700000000000,
            cwd: Some("/home/user/project".to_string()),
            tool_use_id: Some("tu_1".to_string()),
            subagent_id: None,
            trigger_color: None,
            trigger_id: None,
            trigger_name: None,
        };

        let error = create_detected_error(params);

        assert!(!error.id.is_empty());
        assert_eq!(error.session_id, "session-123");
        assert_eq!(error.project_id, "proj-abc");
        assert_eq!(error.file_path, "/path/to/session.jsonl");
        assert_eq!(error.source, "Bash");
        assert_eq!(error.message, "Build failed");
        assert_eq!(error.timestamp, 1700000000000);
        assert_eq!(error.line_number, Some(42));
        assert_eq!(error.tool_use_id, Some("tu_1".to_string()));
        assert_eq!(error.subagent_id, None);
        assert_eq!(error.context.project_name, "my-project");
        assert_eq!(error.context.cwd, Some("/home/user/project".to_string()));
    }

    #[test]
    fn test_create_detected_error_truncates_message() {
        let long_msg = "x".repeat(1000);
        let params = CreateDetectedErrorParams {
            session_id: "s1".to_string(),
            project_id: "p1".to_string(),
            file_path: "/f.jsonl".to_string(),
            project_name: "proj".to_string(),
            line_number: 1,
            source: "Read".to_string(),
            message: long_msg.clone(),
            timestamp: 0,
            cwd: None,
            tool_use_id: None,
            subagent_id: None,
            trigger_color: None,
            trigger_id: None,
            trigger_name: None,
        };

        let error = create_detected_error(params);
        assert!(error.message.ends_with("..."));
        assert!(error.message.len() < long_msg.len());
    }

    #[test]
    fn test_create_detected_error_with_trigger_fields() {
        let params = CreateDetectedErrorParams {
            session_id: "s1".to_string(),
            project_id: "p1".to_string(),
            file_path: "/f.jsonl".to_string(),
            project_name: "proj".to_string(),
            line_number: 10,
            source: "Bash".to_string(),
            message: "Error".to_string(),
            timestamp: 1000,
            cwd: None,
            tool_use_id: Some("tu_x".to_string()),
            subagent_id: Some("agent_1".to_string()),
            trigger_color: Some("red".to_string()),
            trigger_id: Some("trigger_1".to_string()),
            trigger_name: Some("Build Errors".to_string()),
        };

        let error = create_detected_error(params);

        assert_eq!(error.subagent_id, Some("agent_1".to_string()));
        assert_eq!(error.trigger_color, Some("red".to_string()));
        assert_eq!(error.trigger_id, Some("trigger_1".to_string()));
        assert_eq!(error.trigger_name, Some("Build Errors".to_string()));
    }

    #[test]
    fn test_create_detected_error_generates_unique_ids() {
        let params = CreateDetectedErrorParams {
            session_id: "s1".to_string(),
            project_id: "p1".to_string(),
            file_path: "/f.jsonl".to_string(),
            project_name: "proj".to_string(),
            line_number: 1,
            source: "Bash".to_string(),
            message: "err".to_string(),
            timestamp: 0,
            cwd: None,
            tool_use_id: None,
            subagent_id: None,
            trigger_color: None,
            trigger_id: None,
            trigger_name: None,
        };

        let e1 = create_detected_error(params.clone());
        let e2 = create_detected_error(params);

        assert_ne!(e1.id, e2.id);
    }

    #[test]
    fn test_create_detected_error_with_no_cwd() {
        let params = CreateDetectedErrorParams {
            session_id: "s1".to_string(),
            project_id: "p1".to_string(),
            file_path: "/f.jsonl".to_string(),
            project_name: "proj".to_string(),
            line_number: 5,
            source: "Write".to_string(),
            message: "Permission denied".to_string(),
            timestamp: 2000,
            cwd: None,
            tool_use_id: None,
            subagent_id: None,
            trigger_color: None,
            trigger_id: None,
            trigger_name: None,
        };

        let error = create_detected_error(params);
        assert_eq!(error.context.cwd, None);
    }
}
