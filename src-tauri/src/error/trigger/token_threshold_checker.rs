//! Token 阈值触发器检查 —— check_token_threshold_trigger()。

use std::collections::HashMap;

use crate::analysis::tool_extraction::{estimate_tokens, ToolResultInfo};
use crate::analysis::tool_summary_formatter::{format_tokens, get_tool_summary};
use crate::error::error_message_builder::{
    create_detected_error, CreateDetectedErrorParams,
};
use crate::error::trigger_matcher::matches_ignore_patterns;
use crate::types::config::{NotificationTrigger, TriggerMode, TriggerTokenType};
use crate::types::domain::MessageType;
use crate::types::messages::ParsedMessage;
use crate::utils::path_decoder::extract_project_name;

use super::common::parse_timestamp_to_ms;

/// 检查单个 tool_use 块是否超过 token 阈值。
///
/// 为每个超过阈值的 tool_use 返回一个 `DetectedError` 数组。
///
/// Token 计算方式（反映上下文窗口影响）：
/// - 工具调用 token：根据 name + JSON.stringify(input) 估算（进入上下文的部分）
/// - 工具结果 token：根据 tool_result.content 估算（Claude 读取的部分）
/// - 总计 = 调用 + 结果
///
/// # 参数
/// * `message` - 待检查的已解析消息（必须为 assistant 类型）
/// * `trigger` - 通知触发器配置
/// * `tool_result_map` - tool_use ID 到 ToolResultInfo 的映射
/// * `session_id` - 会话标识符
/// * `project_id` - 项目标识符
/// * `file_path` - 源文件路径
/// * `line_number` - 文件中的行号
///
/// # 返回值
/// 每个超过阈值的 tool_use 对应的 `DetectedError` 向量。
pub fn check_token_threshold_trigger(
    message: &ParsedMessage,
    trigger: &NotificationTrigger,
    tool_result_map: &HashMap<String, ToolResultInfo>,
    session_id: &str,
    project_id: &str,
    file_path: &str,
    line_number: u64,
) -> Vec<crate::types::config::DetectedError> {
    let mut errors = Vec::new();

    // 仅检查 token_threshold 模式
    if trigger.mode != TriggerMode::TokenThreshold {
        return errors;
    }

    let threshold = match trigger.token_threshold {
        Some(t) => t as usize,
        _ => return errors,
    };

    // 仅检查包含 tool_use 块的 assistant 消息
    if message.message_type != MessageType::Assistant {
        return errors;
    }

    let token_type = trigger.token_type.as_ref().unwrap_or(&TriggerTokenType::Total);
    let ignore_patterns: &[String] = trigger.ignore_patterns.as_deref().unwrap_or(&[]);
    let timestamp_ms = parse_timestamp_to_ms(&message.timestamp);

    // 从消息中收集所有 tool_use 块，避免重复
    let mut seen_ids: HashMap<String, bool> = HashMap::new();
    let mut tool_use_blocks: Vec<(String, String, serde_json::Value)> = Vec::new();

    // 从 content 数组中查找 tool_use 块
    if let serde_json::Value::Array(ref blocks) = message.content {
        for block in blocks {
            if block.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                let id = block
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let input = block.get("input").cloned().unwrap_or(serde_json::json!({}));

                if !seen_ids.contains_key(&id) {
                    seen_ids.insert(id.clone(), true);
                    tool_use_blocks.push((id, name, input));
                }
            }
        }
    }

    // 同时检查 toolCalls 数组（若存在）
    for tool_call in &message.tool_calls {
        if !seen_ids.contains_key(&tool_call.id) {
            seen_ids.insert(tool_call.id.clone(), true);
            tool_use_blocks.push((
                tool_call.id.clone(),
                tool_call.name.clone(),
                tool_call.input.clone(),
            ));
        }
    }

    if tool_use_blocks.is_empty() {
        return errors;
    }

    // 逐个检查每个 tool_use 块
    for (tool_use_id, tool_use_name, tool_use_input) in &tool_use_blocks {
        // 若指定了工具名称则进行过滤
        if let Some(ref trigger_tool_name) = trigger.tool_name {
            if tool_use_name != trigger_tool_name {
                continue;
            }
        }

        // 直接从 name + input 计算工具调用的 token 数
        let call_text = format!("{}{}", tool_use_name, tool_use_input);
        let tool_call_tokens = estimate_tokens(&serde_json::Value::String(call_text));

        // 计算工具结果的 token 数（Claude 回读的部分）
        let tool_result_tokens = tool_result_map
            .get(tool_use_id)
            .map(|result| estimate_tokens(&result.content))
            .unwrap_or(0);

        // 根据 tokenType 计算 token 数量
        let token_count = match token_type {
            TriggerTokenType::Input => tool_call_tokens,
            TriggerTokenType::Output => tool_result_tokens,
            TriggerTokenType::Total => tool_call_tokens + tool_result_tokens,
        };

        // 检查阈值
        if token_count <= threshold {
            continue;
        }

        // 构建工具摘要信息
        let tool_summary = get_tool_summary(tool_use_name, tool_use_input);

        // 构建包含工具信息和 token 类型的消息
        let token_type_label = match token_type {
            TriggerTokenType::Total => String::new(),
            other => format!(" {}", other.as_str()),
        };
        let token_message = format!(
            "{} - {} : ~{}{} tokens",
            tool_use_name,
            tool_summary,
            format_tokens(token_count),
            token_type_label
        );

        // 检查忽略模式
        if matches_ignore_patterns(&token_message, ignore_patterns) {
            continue;
        }

        errors.push(create_detected_error(CreateDetectedErrorParams {
            session_id: session_id.to_string(),
            project_id: project_id.to_string(),
            file_path: file_path.to_string(),
            project_name: extract_project_name(project_id, message.cwd.as_deref()),
            line_number,
            source: tool_use_name.clone(),
            message: token_message,
            timestamp: timestamp_ms,
            cwd: message.cwd.clone(),
            tool_use_id: Some(tool_use_id.clone()),
            subagent_id: None,
            trigger_color: trigger.color.clone(),
            trigger_id: Some(trigger.id.clone()),
            trigger_name: Some(trigger.name.clone()),
        }));
    }

    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::config::{TriggerContentType, TriggerMode, TriggerTokenType};
    use crate::types::messages::{ToolCall, ToolResult};
    use serde_json::json;

    fn make_token_threshold_trigger(threshold: u64) -> NotificationTrigger {
        NotificationTrigger {
            id: "token-trigger".to_string(),
            name: "Token Threshold".to_string(),
            enabled: true,
            content_type: TriggerContentType::ToolResult,
            tool_name: None,
            is_builtin: None,
            ignore_patterns: None,
            mode: TriggerMode::TokenThreshold,
            require_error: None,
            match_field: None,
            match_pattern: None,
            token_threshold: Some(threshold),
            token_type: Some(TriggerTokenType::Total),
            repository_ids: None,
            color: None,
        }
    }

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

    fn make_assistant_message(
        content: serde_json::Value,
        tool_calls: Vec<ToolCall>,
        tool_results: Vec<ToolResult>,
    ) -> ParsedMessage {
        ParsedMessage {
            uuid: "test-uuid".to_string(),
            parent_uuid: None,
            message_type: MessageType::Assistant,
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
    fn test_check_token_threshold_trigger_exceeds() {
        // Create content with enough text to exceed a low threshold
        let large_input = json!({"file_path": "/very/long/path/to/some/deeply/nested/file/that/has/a/very/long/name/and/this/should/generate/lots/of/tokens.rs"});
        let large_result = json!("a".repeat(2000));

        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Read",
                "input": large_input
            }
        ]);

        let msg = make_assistant_message(
            content,
            vec![make_tool_call("tu1", "Read", json!({"file_path": "/test.rs"}))],
            vec![make_tool_result("tu1", large_result, false)],
        );

        let trigger = make_token_threshold_trigger(100);

        let mut tool_result_map = HashMap::new();
        tool_result_map.insert(
            "tu1".to_string(),
            ToolResultInfo {
                content: json!("a".repeat(2000)),
                is_error: false,
            },
        );

        let errors = check_token_threshold_trigger(
            &msg,
            &trigger,
            &tool_result_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].source, "Read");
    }

    #[test]
    fn test_check_token_threshold_trigger_below() {
        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Bash",
                "input": {"command": "ls"}
            }
        ]);

        let msg = make_assistant_message(
            content,
            vec![make_tool_call("tu1", "Bash", json!({"command": "ls"}))],
            vec![make_tool_result("tu1", json!("done"), false)],
        );

        let trigger = make_token_threshold_trigger(100_000);

        let tool_result_map = HashMap::new();

        let errors = check_token_threshold_trigger(
            &msg,
            &trigger,
            &tool_result_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert!(errors.is_empty());
    }

    #[test]
    fn test_check_token_threshold_trigger_wrong_mode() {
        let trigger = make_trigger(TriggerMode::ErrorStatus, TriggerContentType::ToolResult);
        let msg = make_assistant_message(json!("test"), vec![], vec![]);
        let tool_result_map = HashMap::new();

        let errors = check_token_threshold_trigger(
            &msg,
            &trigger,
            &tool_result_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert!(errors.is_empty());
    }

    #[test]
    fn test_check_token_threshold_trigger_non_assistant() {
        let trigger = make_token_threshold_trigger(1);
        let msg = ParsedMessage {
            uuid: "user-1".to_string(),
            parent_uuid: None,
            message_type: MessageType::User,
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
            role: Some("user".to_string()),
            content: json!("hello"),
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

        let errors = check_token_threshold_trigger(
            &msg,
            &trigger,
            &HashMap::new(),
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert!(errors.is_empty());
    }

    #[test]
    fn test_check_token_threshold_trigger_tool_name_filter() {
        let trigger = NotificationTrigger {
            tool_name: Some("Read".to_string()),
            ..make_token_threshold_trigger(100)
        };

        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Bash",
                "input": {"command": "a".repeat(1000)}
            }
        ]);

        let msg = make_assistant_message(
            content,
            vec![make_tool_call("tu1", "Bash", json!({"command": "a".repeat(1000)}))],
            vec![],
        );

        let tool_result_map = HashMap::new();

        let errors = check_token_threshold_trigger(
            &msg,
            &trigger,
            &tool_result_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        // Bash should be filtered out by tool_name=Read
        assert!(errors.is_empty());
    }

    #[test]
    fn test_check_token_threshold_trigger_ignore_pattern() {
        let trigger = NotificationTrigger {
            ignore_patterns: Some(vec!["Read.*test\\.rs".to_string()]),
            ..make_token_threshold_trigger(100)
        };

        let large_content = "a".repeat(2000);
        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Read",
                "input": {"file_path": "/test.rs"}
            }
        ]);

        let msg = make_assistant_message(
            content,
            vec![make_tool_call("tu1", "Read", json!({"file_path": "/test.rs"}))],
            vec![make_tool_result("tu1", json!(large_content), false)],
        );

        let mut tool_result_map = HashMap::new();
        tool_result_map.insert(
            "tu1".to_string(),
            ToolResultInfo {
                content: json!(large_content),
                is_error: false,
            },
        );

        let errors = check_token_threshold_trigger(
            &msg,
            &trigger,
            &tool_result_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        // Should be ignored due to pattern match
        assert!(errors.is_empty());
    }

    #[test]
    fn test_check_token_threshold_trigger_zero_threshold() {
        let trigger = make_token_threshold_trigger(0);
        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Read",
                "input": {"file_path": "/test.rs"}
            }
        ]);
        let msg = make_assistant_message(
            content,
            vec![make_tool_call("tu1", "Read", json!({"file_path": "/test.rs"}))],
            vec![make_tool_result("tu1", json!("some content"), false)],
        );
        let tool_result_map = HashMap::new();

        let errors = check_token_threshold_trigger(
            &msg,
            &trigger,
            &tool_result_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        // Zero threshold means any token usage triggers notification
        assert!(!errors.is_empty());
    }

    #[test]
    fn test_check_token_threshold_trigger_multiple_tools() {
        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Read",
                "input": {"file_path": "/test.rs"}
            },
            {
                "type": "tool_use",
                "id": "tu2",
                "name": "Read",
                "input": {"file_path": "/other.rs"}
            }
        ]);

        let large_content = "a".repeat(2000);
        let msg = make_assistant_message(
            content,
            vec![
                make_tool_call("tu1", "Read", json!({"file_path": "/test.rs"})),
                make_tool_call("tu2", "Read", json!({"file_path": "/other.rs"})),
            ],
            vec![
                make_tool_result("tu1", json!(large_content.clone()), false),
                make_tool_result("tu2", json!(large_content), false),
            ],
        );

        let trigger = make_token_threshold_trigger(100);

        let mut tool_result_map = HashMap::new();
        tool_result_map.insert(
            "tu1".to_string(),
            ToolResultInfo {
                content: json!("a".repeat(2000)),
                is_error: false,
            },
        );
        tool_result_map.insert(
            "tu2".to_string(),
            ToolResultInfo {
                content: json!("a".repeat(2000)),
                is_error: false,
            },
        );

        let errors = check_token_threshold_trigger(
            &msg,
            &trigger,
            &tool_result_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn test_check_token_threshold_trigger_no_duplicate_from_content_and_tool_calls() {
        // Same tool_use in both content and toolCalls -- should not duplicate
        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Read",
                "input": {"file_path": "/test.rs"}
            }
        ]);

        let large_content = "a".repeat(2000);
        let msg = make_assistant_message(
            content,
            vec![make_tool_call("tu1", "Read", json!({"file_path": "/test.rs"}))],
            vec![make_tool_result("tu1", json!(large_content), false)],
        );

        let trigger = make_token_threshold_trigger(100);

        let mut tool_result_map = HashMap::new();
        tool_result_map.insert(
            "tu1".to_string(),
            ToolResultInfo {
                content: json!("a".repeat(2000)),
                is_error: false,
            },
        );

        let errors = check_token_threshold_trigger(
            &msg,
            &trigger,
            &tool_result_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        // Should not duplicate -- only one error for tu1
        assert_eq!(errors.len(), 1);
    }
}
