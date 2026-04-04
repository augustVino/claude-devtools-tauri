//! 工具调用触发器检查 —— check_tool_use_trigger()。

use crate::error::error_message_builder::{
    create_detected_error, CreateDetectedErrorParams,
};
use crate::error::trigger_matcher::{
    extract_tool_use_field, get_content_blocks, matches_ignore_patterns, matches_pattern,
};
use crate::types::config::NotificationTrigger;
use crate::types::domain::MessageType;
use crate::types::messages::{ParsedMessage, ToolCall};
use crate::utils::path_decoder::extract_project_name;

use super::common::{parse_timestamp_to_ms, truncate_content};

/// 检查 tool_use 是否匹配触发器。
///
/// 遍历内容块查找 `tool_use` 类型的块，
/// 然后检查工具名称过滤、模式匹配和忽略模式。
///
/// # 参数
/// * `message` - 待检查的已解析消息（必须为 assistant 类型）
/// * `trigger` - 通知触发器配置
/// * `session_id` - 会话标识符
/// * `project_id` - 项目标识符
/// * `file_path` - 源文件路径
/// * `line_number` - 文件中的行号
///
/// # 返回值
/// 若触发器匹配则返回 `DetectedError`，否则返回 `None`。
pub fn check_tool_use_trigger(
    message: &ParsedMessage,
    trigger: &NotificationTrigger,
    session_id: &str,
    project_id: &str,
    file_path: &str,
    line_number: u64,
) -> Option<crate::types::config::DetectedError> {
    if message.message_type != MessageType::Assistant {
        return None;
    }

    let content_blocks = get_content_blocks(message);
    let ignore_patterns: &[String] = trigger.ignore_patterns.as_deref().unwrap_or(&[]);
    let timestamp_ms = parse_timestamp_to_ms(&message.timestamp);

    for block in &content_blocks {
        let block_type = block.get("type").and_then(|v| v.as_str());
        if block_type != Some("tool_use") {
            continue;
        }

        // 从 JSON 块中提取 tool_use 字段
        let tool_use_id = block.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let tool_use_name = block.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let tool_use_input = block.get("input").cloned().unwrap_or(serde_json::Value::Null);

        // 构建 ToolCall 用于字段提取
        let tool_call = ToolCall {
            id: tool_use_id.to_string(),
            name: tool_use_name.to_string(),
            input: tool_use_input,
            is_task: false,
            task_description: None,
            task_subagent_type: None,
        };

        // 若指定了工具名称则进行过滤
        if let Some(ref trigger_tool_name) = trigger.tool_name {
            if tool_use_name != trigger_tool_name {
                continue;
            }
        }

        // 根据 matchField 提取待匹配的字段
        // 若未指定 matchField（如"任意工具"），则匹配整个 input JSON
        let field_value = if trigger.match_field.is_some() {
            extract_tool_use_field(&tool_call, trigger.match_field.as_deref())
        } else {
            // 匹配整个 input JSON
            let input_str = serde_json::to_string(&tool_call.input).unwrap_or_default();
            if input_str.is_empty() || input_str == "null" {
                None
            } else {
                Some(input_str)
            }
        };

        let field_value = match field_value {
            Some(v) => v,
            None => continue,
        };

        // 检查匹配模式
        if let Some(ref pattern) = trigger.match_pattern {
            if !matches_pattern(&field_value, pattern) {
                continue;
            }
        }

        // 检查忽略模式
        if matches_ignore_patterns(&field_value, ignore_patterns) {
            continue;
        }

        // 匹配成功！
        let field_label = trigger
            .match_field
            .as_deref()
            .unwrap_or("tool_use");
        let truncated = truncate_content(&field_value, 200);
        let error_msg = format!("{}: {}", field_label, truncated);

        return Some(create_detected_error(CreateDetectedErrorParams {
            session_id: session_id.to_string(),
            project_id: project_id.to_string(),
            file_path: file_path.to_string(),
            project_name: extract_project_name(project_id, message.cwd.as_deref()),
            line_number,
            source: tool_use_name.to_string(),
            message: error_msg,
            timestamp: timestamp_ms,
            cwd: message.cwd.clone(),
            tool_use_id: Some(tool_use_id.to_string()),
            subagent_id: None,
            trigger_color: trigger.color.clone(),
            trigger_id: Some(trigger.id.clone()),
            trigger_name: Some(trigger.name.clone()),
        }));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::config::{TriggerContentType, TriggerMode};
    use crate::types::messages::ParsedMessage;
    use serde_json::json;

    fn make_tool_use_trigger(
        tool_name: Option<&str>,
        pattern: Option<&str>,
    ) -> NotificationTrigger {
        NotificationTrigger {
            id: "tool-use-trigger".to_string(),
            name: "Tool Use Trigger".to_string(),
            enabled: true,
            content_type: TriggerContentType::ToolUse,
            tool_name: tool_name.map(String::from),
            is_builtin: None,
            ignore_patterns: None,
            mode: TriggerMode::ContentMatch,
            require_error: None,
            match_field: None,
            match_pattern: pattern.map(String::from),
            token_threshold: None,
            token_type: None,
            repository_ids: None,
            color: None,
        }
    }

    fn make_assistant_message(
        content: serde_json::Value,
        tool_calls: Vec<ToolCall>,
        tool_results: Vec<crate::types::messages::ToolResult>,
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

    #[test]
    fn test_check_tool_use_trigger_match() {
        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Bash",
                "input": {"command": "rm -rf /"}
            }
        ]);

        let msg = make_assistant_message(content, vec![], vec![]);
        let trigger = make_tool_use_trigger(Some("Bash"), Some("rm"));

        let result = check_tool_use_trigger(
            &msg,
            &trigger,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert!(result.is_some());
        let error = result.unwrap();
        assert_eq!(error.source, "Bash");
        assert!(error.message.contains("tool_use"));
    }

    #[test]
    fn test_check_tool_use_trigger_no_match_pattern() {
        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Bash",
                "input": {"command": "rm -rf /"}
            }
        ]);

        let msg = make_assistant_message(content, vec![], vec![]);
        let trigger = make_tool_use_trigger(Some("Bash"), Some("git.*push"));

        let result = check_tool_use_trigger(
            &msg,
            &trigger,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert!(result.is_none());
    }

    #[test]
    fn test_check_tool_use_trigger_wrong_tool_name() {
        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Read",
                "input": {"file_path": "/test.rs"}
            }
        ]);

        let msg = make_assistant_message(content, vec![], vec![]);
        let trigger = make_tool_use_trigger(Some("Bash"), Some("rm"));

        let result = check_tool_use_trigger(
            &msg,
            &trigger,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert!(result.is_none());
    }

    #[test]
    fn test_check_tool_use_trigger_ignored() {
        let trigger = NotificationTrigger {
            ignore_patterns: Some(vec!["safe.*command".to_string()]),
            ..make_tool_use_trigger(None, Some("echo"))
        };

        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Bash",
                "input": {"command": "safe command here"}
            }
        ]);

        let msg = make_assistant_message(content, vec![], vec![]);

        let result = check_tool_use_trigger(
            &msg,
            &trigger,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert!(result.is_none());
    }

    #[test]
    fn test_check_tool_use_trigger_non_assistant() {
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

        let trigger = make_tool_use_trigger(None, Some("anything"));
        let result = check_tool_use_trigger(
            &msg,
            &trigger,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert!(result.is_none());
    }

    #[test]
    fn test_check_tool_use_trigger_with_match_field() {
        let trigger = NotificationTrigger {
            match_field: Some("command".to_string()),
            ..make_tool_use_trigger(None, Some("rm"))
        };

        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Bash",
                "input": {"command": "rm -rf /tmp/test"}
            }
        ]);

        let msg = make_assistant_message(content, vec![], vec![]);

        let result = check_tool_use_trigger(
            &msg,
            &trigger,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert!(result.is_some());
        let error = result.unwrap();
        assert!(error.message.contains("command"));
    }

    #[test]
    fn test_check_tool_use_trigger_any_tool_no_pattern() {
        // No tool name, no pattern -- should match any tool_use block
        let trigger = make_tool_use_trigger(None, None);

        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Read",
                "input": {"file_path": "/test.rs"}
            }
        ]);

        let msg = make_assistant_message(content, vec![], vec![]);

        let result = check_tool_use_trigger(
            &msg,
            &trigger,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert!(result.is_some());
    }
}
