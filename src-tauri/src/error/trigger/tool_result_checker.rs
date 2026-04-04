//! 工具结果触发器检查 —— check_tool_result_trigger()。

use std::collections::HashMap;

use crate::analysis::tool_extraction::{extract_tool_results, ToolResultInfo, ToolUseInfo};
use crate::error::error_message_builder::{
    create_detected_error, extract_error_message, find_tool_name_by_tool_use_id,
    CreateDetectedErrorParams,
};
use crate::error::trigger_matcher::{
    matches_ignore_patterns, matches_pattern,
};
use crate::types::config::NotificationTrigger;
use crate::types::messages::ParsedMessage;
use crate::utils::path_decoder::extract_project_name;

use super::common::{parse_timestamp_to_ms, truncate_content};

/// 检查 tool_result 是否匹配触发器。
///
/// 处理两种模式：
/// - `require_error`：仅当工具结果的 `is_error` 为 true 时匹配
/// - 内容匹配：将工具结果内容与触发器 pattern 进行匹配
///
/// # 参数
/// * `message` - 待检查的已解析消息
/// * `trigger` - 通知触发器配置
/// * `tool_use_map` - tool_use ID 到 ToolUseInfo 的映射
/// * `session_id` - 会话标识符
/// * `project_id` - 项目标识符
/// * `file_path` - 源文件路径
/// * `line_number` - 文件中的行号
///
/// # 返回值
/// 若触发器匹配则返回 `DetectedError`，否则返回 `None`。
pub fn check_tool_result_trigger(
    message: &ParsedMessage,
    trigger: &NotificationTrigger,
    tool_use_map: &HashMap<String, ToolUseInfo>,
    session_id: &str,
    project_id: &str,
    file_path: &str,
    line_number: u64,
) -> Option<crate::types::config::DetectedError> {
    let tool_results = extract_tool_results(message, &|msg, id| {
        find_tool_name_by_tool_use_id(msg, id)
    });

    let ignore_patterns: &[String] = trigger.ignore_patterns.as_deref().unwrap_or(&[]);
    let timestamp_ms = parse_timestamp_to_ms(&message.timestamp);

    for result in &tool_results {
        // 若 requireError 为 true，仅匹配 is_error 为 true 的结果
        if trigger.require_error.unwrap_or(false) {
            if !result.is_error {
                continue;
            }

            // 提取错误消息用于忽略模式检查
            let error_message = extract_error_message(result);

            // 检查忽略模式 —— 若任意匹配则跳过此错误
            if matches_ignore_patterns(&error_message, ignore_patterns) {
                continue;
            }

            // 创建已检测的错误
            return Some(create_detected_error(CreateDetectedErrorParams {
                session_id: session_id.to_string(),
                project_id: project_id.to_string(),
                file_path: file_path.to_string(),
                project_name: extract_project_name(project_id, message.cwd.as_deref()),
                line_number,
                source: result
                    .tool_name
                    .clone()
                    .unwrap_or_else(|| "tool_result".to_string()),
                message: error_message,
                timestamp: timestamp_ms,
                cwd: message.cwd.clone(),
                tool_use_id: Some(result.tool_use_id.clone()),
                subagent_id: None,
                trigger_color: trigger.color.clone(),
                trigger_id: Some(trigger.id.clone()),
                trigger_name: Some(trigger.name.clone()),
            }));
        }

        // 非错误 tool_result 触发器（若指定了 toolName）
        if let Some(ref tool_name) = trigger.tool_name {
            let tool_use = tool_use_map.get(&result.tool_use_id);
            if let Some(info) = tool_use {
                if info.name != *tool_name {
                    continue;
                }
            } else {
                continue;
            }

            // 当 matchField 为 .content. 时，匹配内容
            if trigger.match_field.as_deref() == Some("content") {
                if let Some(ref pattern) = trigger.match_pattern {
                    let content = match &result.content {
                        serde_json::Value::String(s) => s.clone(),
                        other => serde_json::to_string(other).unwrap_or_default(),
                    };

                    if !matches_pattern(&content, pattern) {
                        continue;
                    }
                    if matches_ignore_patterns(&content, ignore_patterns) {
                        continue;
                    }

                    let truncated = truncate_content(&content, 200);
                    let error_msg = format!("Tool result matched: {}", truncated);

                    return Some(create_detected_error(CreateDetectedErrorParams {
                        session_id: session_id.to_string(),
                        project_id: project_id.to_string(),
                        file_path: file_path.to_string(),
                        project_name: extract_project_name(project_id, message.cwd.as_deref()),
                        line_number,
                        source: tool_name.clone(),
                        message: error_msg,
                        timestamp: timestamp_ms,
                        cwd: message.cwd.clone(),
                        tool_use_id: Some(result.tool_use_id.clone()),
                        subagent_id: None,
                        trigger_color: trigger.color.clone(),
                        trigger_id: Some(trigger.id.clone()),
                        trigger_name: Some(trigger.name.clone()),
                    }));
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::tool_extraction::ToolUseInfo;
    use crate::types::config::{TriggerContentType, TriggerMode};
    use crate::types::messages::{ToolCall, ToolResult};
    use serde_json::json;

    fn make_trigger(mode: TriggerMode, content_type: TriggerContentType) -> NotificationTrigger {
        NotificationTrigger {
            id: "trigger-1".to_string(),
            name: "Test Trigger".to_string(),
            enabled: true,
            content_type,
            tool_name: None,
            is_builtin: None,
            ignore_patterns: None,
            mode,
            require_error: None,
            match_field: None,
            match_pattern: None,
            token_threshold: None,
            token_type: None,
            repository_ids: None,
            color: None,
        }
    }

    fn make_error_trigger() -> NotificationTrigger {
        NotificationTrigger {
            id: "error-trigger".to_string(),
            name: "Error Trigger".to_string(),
            enabled: true,
            content_type: TriggerContentType::ToolResult,
            tool_name: None,
            is_builtin: None,
            ignore_patterns: None,
            mode: TriggerMode::ErrorStatus,
            require_error: Some(true),
            match_field: None,
            match_pattern: None,
            token_threshold: None,
            token_type: None,
            repository_ids: None,
            color: Some("red".to_string()),
        }
    }

    fn make_content_match_trigger(tool_name: &str, pattern: &str) -> NotificationTrigger {
        NotificationTrigger {
            id: "content-trigger".to_string(),
            name: "Content Match".to_string(),
            enabled: true,
            content_type: TriggerContentType::ToolResult,
            tool_name: Some(tool_name.to_string()),
            is_builtin: None,
            ignore_patterns: None,
            mode: TriggerMode::ContentMatch,
            require_error: None,
            match_field: Some("content".to_string()),
            match_pattern: Some(pattern.to_string()),
            token_threshold: None,
            token_type: None,
            repository_ids: None,
            color: None,
        }
    }

    fn make_assistant_message(
        content: serde_json::Value,
        tool_calls: Vec<ToolCall>,
        tool_results: Vec<ToolResult>,
    ) -> ParsedMessage {
        ParsedMessage {
            uuid: "test-uuid".to_string(),
            parent_uuid: None,
            message_type: crate::types::domain::MessageType::Assistant,
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
            role: Some("assistant".to_string()),
            content,
            usage: None,
            model: None,
            cwd: Some("/home/user/project".to_string()),
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

    fn make_tool_call(id: &str, name: &str, input: serde_json::Value) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            input,
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
    fn test_check_tool_result_trigger_error_status_match() {
        let msg = make_assistant_message(
            json!("test"),
            vec![make_tool_call(
                "tc1",
                "Bash",
                json!({"command": "npm build"}),
            )],
            vec![make_tool_result("tc1", json!("Build failed: error E0425"), true)],
        );

        let trigger = make_error_trigger();
        let tool_use_map = HashMap::new();

        let result = check_tool_result_trigger(
            &msg,
            &trigger,
            &tool_use_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            42,
        );

        assert!(result.is_some());
        let error = result.unwrap();
        assert_eq!(error.source, "Bash");
        assert_eq!(error.message, "Build failed: error E0425");
        assert_eq!(error.trigger_id, Some("error-trigger".to_string()));
        assert_eq!(error.trigger_color, Some("red".to_string()));
    }

    #[test]
    fn test_check_tool_result_trigger_error_status_no_error() {
        let msg = make_assistant_message(
            json!("test"),
            vec![make_tool_call(
                "tc1",
                "Bash",
                json!({"command": "npm build"}),
            )],
            vec![make_tool_result("tc1", json!("Build succeeded"), false)],
        );

        let trigger = make_error_trigger();
        let tool_use_map = HashMap::new();

        let result = check_tool_result_trigger(
            &msg,
            &trigger,
            &tool_use_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            42,
        );

        assert!(result.is_none());
    }

    #[test]
    fn test_check_tool_result_trigger_error_status_ignored() {
        let trigger = NotificationTrigger {
            ignore_patterns: Some(vec!["E0425".to_string()]),
            ..make_error_trigger()
        };

        let msg = make_assistant_message(
            json!("test"),
            vec![make_tool_call(
                "tc1",
                "Bash",
                json!({"command": "npm build"}),
            )],
            vec![make_tool_result("tc1", json!("Build failed: error E0425"), true)],
        );

        let tool_use_map = HashMap::new();

        let result = check_tool_result_trigger(
            &msg,
            &trigger,
            &tool_use_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            42,
        );

        assert!(result.is_none());
    }

    #[test]
    fn test_check_tool_result_trigger_content_match() {
        let trigger = make_content_match_trigger("Bash", "failed");

        let msg = make_assistant_message(
            json!("test"),
            vec![make_tool_call(
                "tc1",
                "Bash",
                json!({"command": "npm build"}),
            )],
            vec![make_tool_result("tc1", json!("Build failed"), false)],
        );

        let mut tool_use_map = HashMap::new();
        tool_use_map.insert(
            "tc1".to_string(),
            ToolUseInfo {
                name: "Bash".to_string(),
                input: json!({"command": "npm build"}),
            },
        );

        let result = check_tool_result_trigger(
            &msg,
            &trigger,
            &tool_use_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            42,
        );

        assert!(result.is_some());
        let error = result.unwrap();
        assert_eq!(error.source, "Bash");
        assert!(error.message.contains("Tool result matched"));
    }

    #[test]
    fn test_check_tool_result_trigger_content_match_wrong_tool() {
        let trigger = make_content_match_trigger("Read", "failed");

        let msg = make_assistant_message(
            json!("test"),
            vec![make_tool_call(
                "tc1",
                "Bash",
                json!({"command": "npm build"}),
            )],
            vec![make_tool_result("tc1", json!("Build failed"), false)],
        );

        let mut tool_use_map = HashMap::new();
        tool_use_map.insert(
            "tc1".to_string(),
            ToolUseInfo {
                name: "Bash".to_string(),
                input: json!({"command": "npm build"}),
            },
        );

        let result = check_tool_result_trigger(
            &msg,
            &trigger,
            &tool_use_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            42,
        );

        assert!(result.is_none());
    }

    #[test]
    fn test_check_tool_result_trigger_content_match_pattern_miss() {
        let trigger = make_content_match_trigger("Bash", "success");

        let msg = make_assistant_message(
            json!("test"),
            vec![make_tool_call(
                "tc1",
                "Bash",
                json!({"command": "npm build"}),
            )],
            vec![make_tool_result("tc1", json!("Build failed"), false)],
        );

        let mut tool_use_map = HashMap::new();
        tool_use_map.insert(
            "tc1".to_string(),
            ToolUseInfo {
                name: "Bash".to_string(),
                input: json!({"command": "npm build"}),
            },
        );

        let result = check_tool_result_trigger(
            &msg,
            &trigger,
            &tool_use_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            42,
        );

        assert!(result.is_none());
    }

    #[test]
    fn test_check_tool_result_trigger_no_results() {
        let msg = make_assistant_message(json!("test"), vec![], vec![]);
        let trigger = make_error_trigger();
        let tool_use_map = HashMap::new();

        let result = check_tool_result_trigger(
            &msg,
            &trigger,
            &tool_use_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            42,
        );

        assert!(result.is_none());
    }
}
