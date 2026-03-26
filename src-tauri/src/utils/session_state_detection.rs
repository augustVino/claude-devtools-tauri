//! 会话状态检测工具模块，用于判断会话是否仍在进行中。
//!
//! 当最后一个"结束事件"之后存在 AI 相关活动（thinking、tool_use、tool_result）时，
//! 会话被视为"进行中"。
//!
//! 结束事件：text_output、interruption、exit_plan_mode。
//! 进行中指示器：thinking、tool_use、tool_result。

use crate::types::domain::MessageType;
use crate::types::messages::ParsedMessage;

/// 活动类型枚举，用于跟踪会话状态。
#[derive(Debug, Clone, PartialEq)]
enum ActivityType {
    TextOutput,
    Thinking,
    ToolUse,
    ToolResult,
    Interruption,
    ExitPlanMode,
}

/// 活动条目，包含类型和顺序索引。
struct Activity {
    kind: ActivityType,
    index: usize,
}

/// 检查 toolUseResult 值是否表示用户拒绝了工具调用。
fn is_tool_use_rejection(tool_use_result: &serde_json::Value) -> bool {
    tool_use_result.as_str() == Some("User rejected tool use")
}

/// 检查 tool_use 块是否为 SendMessage 的 shutdown_response 且 approve 为 true。
fn is_shutdown_response(name: &str, input: &serde_json::Value) -> bool {
    name == "SendMessage"
        && input.get("type").and_then(|v| v.as_str()) == Some("shutdown_response")
        && input.get("approve").and_then(|v| v.as_bool()) == Some(true)
}

/// 检查活动序列是否表示会话仍在进行中。
///
/// 当最后一个结束事件（text_output、interruption、exit_plan_mode）之后
/// 存在任何 AI 活动（thinking、tool_use、tool_result）时，会话仍在进行中。
fn is_ongoing_from_activities(activities: &[Activity]) -> bool {
    if activities.is_empty() {
        return false;
    }

    // 查找最后一个"结束事件"的索引
    let last_ending_index = activities
        .iter()
        .rev()
        .find(|a| {
            matches!(
                a.kind,
                ActivityType::TextOutput
                    | ActivityType::Interruption
                    | ActivityType::ExitPlanMode
            )
        })
        .map(|a| a.index);

    match last_ending_index {
        None => {
            // 未找到结束事件 — 只要存在任何 AI 活动即视为进行中
            activities.iter().any(|a| {
                matches!(
                    a.kind,
                    ActivityType::Thinking | ActivityType::ToolUse | ActivityType::ToolResult
                )
            })
        }
        Some(ending_idx) => {
            // 检查最后一个结束事件之后是否存在 AI 活动
            activities.iter().any(|a| {
                a.index > ending_idx
                    && matches!(
                        a.kind,
                        ActivityType::Thinking | ActivityType::ToolUse | ActivityType::ToolResult
                    )
            })
        }
    }
}

/// 检查消息序列是否表示会话仍在进行中（AI 响应尚未完成）。
///
/// 此逻辑镜像 Electron 端的 `checkMessagesOngoing`：
/// - 通过顺序索引跟踪结束事件和进行中指示器。
/// - ExitPlanMode 作为特殊结束工具处理，
///   SendMessage shutdown_response 的调用和结果均为结束事件。
/// - 用户拒绝的工具调用视为中断（结束事件）。
/// - "[Request interrupted by user" 消息视为中断（结束事件）。
pub fn check_messages_ongoing(messages: &[ParsedMessage]) -> bool {
    let mut activities: Vec<Activity> = Vec::new();
    let mut activity_index: usize = 0;
    // 跟踪 shutdown_response 类型的 tool_use ID，
    // 以便其 tool_result 也被标记为结束事件
    let mut shutdown_tool_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    for msg in messages {
        if msg.message_type == MessageType::Assistant {
            // 处理助手消息的内容块
            if let serde_json::Value::Array(ref blocks) = msg.content {
                for block in blocks {
                    let block_type = block.get("type").and_then(|v| v.as_str());

                    match block_type {
                        Some("thinking") => {
                            if block.get("thinking").is_some() {
                                activities.push(Activity {
                                    kind: ActivityType::Thinking,
                                    index: activity_index,
                                });
                                activity_index += 1;
                            }
                        }
                        Some("tool_use") => {
                            let name = block
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let id = block
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");

                            if name == "ExitPlanMode" {
                                // ExitPlanMode 是特殊的结束工具
                                activities.push(Activity {
                                    kind: ActivityType::ExitPlanMode,
                                    index: activity_index,
                                });
                                activity_index += 1;
                            } else if let Some(input) = block.get("input") {
                                if is_shutdown_response(name, input) {
                                    shutdown_tool_ids.insert(id.to_string());
                                    activities.push(Activity {
                                        kind: ActivityType::Interruption,
                                        index: activity_index,
                                    });
                                    activity_index += 1;
                                } else {
                                    activities.push(Activity {
                                        kind: ActivityType::ToolUse,
                                        index: activity_index,
                                    });
                                    activity_index += 1;
                                }
                            } else {
                                activities.push(Activity {
                                    kind: ActivityType::ToolUse,
                                    index: activity_index,
                                });
                                activity_index += 1;
                            }
                        }
                        Some("text") => {
                            let text = block.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            if !text.trim().is_empty() {
                                activities.push(Activity {
                                    kind: ActivityType::TextOutput,
                                    index: activity_index,
                                });
                                activity_index += 1;
                            }
                        }
                        _ => {}
                    }
                }
            }
        } else if msg.message_type == MessageType::User {
            // 检查是否为用户拒绝的工具调用（结束事件）
            let is_rejection = msg
                .tool_use_result
                .as_ref()
                .map(is_tool_use_rejection)
                .unwrap_or(false);

            // 检查用户消息中的工具结果和中断
            if let serde_json::Value::Array(ref blocks) = msg.content {
                for block in blocks {
                    let block_type = block.get("type").and_then(|v| v.as_str());

                    match block_type {
                        Some("tool_result") => {
                            let tool_use_id = block
                                .get("tool_use_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");

                            if shutdown_tool_ids.contains(tool_use_id) {
                                // Shutdown 工具结果 = 结束事件
                                activities.push(Activity {
                                    kind: ActivityType::Interruption,
                                    index: activity_index,
                                });
                                activity_index += 1;
                            } else if is_rejection {
                                // 用户拒绝 = 结束事件
                                activities.push(Activity {
                                    kind: ActivityType::Interruption,
                                    index: activity_index,
                                });
                                activity_index += 1;
                            } else {
                                activities.push(Activity {
                                    kind: ActivityType::ToolResult,
                                    index: activity_index,
                                });
                                activity_index += 1;
                            }
                        }
                        Some("text") => {
                            let text = block.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            if text.starts_with("[Request interrupted by user") {
                                activities.push(Activity {
                                    kind: ActivityType::Interruption,
                                    index: activity_index,
                                });
                                activity_index += 1;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    is_ongoing_from_activities(&activities)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::jsonl::UsageMetadata;
    use crate::types::messages::{ParsedMessage, ToolCall, ToolResult};

    fn make_user_msg(
        uuid: &str,
        is_meta: bool,
        content: serde_json::Value,
        tool_use_result: Option<serde_json::Value>,
    ) -> ParsedMessage {
        ParsedMessage {
            uuid: uuid.to_string(),
            parent_uuid: None,
            message_type: MessageType::User,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            role: Some("user".to_string()),
            content,
            usage: None,
            model: None,
            cwd: None,
            git_branch: None,
            agent_id: None,
            is_sidechain: false,
            is_meta,
            user_type: None,
            tool_calls: vec![],
            tool_results: vec![],
            source_tool_use_id: None,
            source_tool_assistant_uuid: None,
            tool_use_result,
            is_compact_summary: None,
            request_id: None,
        }
    }

    fn make_assistant_msg(uuid: &str, content: serde_json::Value) -> ParsedMessage {
        ParsedMessage {
            uuid: uuid.to_string(),
            parent_uuid: None,
            message_type: MessageType::Assistant,
            timestamp: "2026-01-01T00:00:01Z".to_string(),
            role: Some("assistant".to_string()),
            content,
            usage: Some(UsageMetadata {
                input_tokens: 100,
                output_tokens: 50,
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            }),
            model: Some("claude-3-opus".to_string()),
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
        }
    }

    #[test]
    fn test_empty_messages_not_ongoing() {
        let messages: Vec<ParsedMessage> = vec![];
        assert!(!check_messages_ongoing(&messages));
    }

    #[test]
    fn test_text_output_only_not_ongoing() {
        let messages = vec![make_assistant_msg(
            "a1",
            serde_json::json!([{"type": "text", "text": "Hello world"}]),
        )];
        assert!(!check_messages_ongoing(&messages));
    }

    #[test]
    fn test_thinking_after_text_is_ongoing() {
        let messages = vec![
            make_assistant_msg(
                "a1",
                serde_json::json!([
                    {"type": "thinking", "thinking": "hmm", "signature": "sig"},
                    {"type": "text", "text": "Done"}
                ]),
            ),
            make_assistant_msg(
                "a2",
                serde_json::json!([
                    {"type": "thinking", "thinking": "more...", "signature": "sig"}
                ]),
            ),
        ];
        assert!(check_messages_ongoing(&messages));
    }

    #[test]
    fn test_tool_use_after_text_is_ongoing() {
        let messages = vec![
            make_assistant_msg(
                "a1",
                serde_json::json!([{"type": "text", "text": "Let me check"}]),
            ),
            make_assistant_msg(
                "a2",
                serde_json::json!([
                    {"type": "tool_use", "id": "tu_1", "name": "Read", "input": {}}
                ]),
            ),
        ];
        assert!(check_messages_ongoing(&messages));
    }

    #[test]
    fn test_tool_result_after_text_is_ongoing() {
        let messages = vec![
            make_assistant_msg(
                "a1",
                serde_json::json!([
                    {"type": "tool_use", "id": "tu_1", "name": "Read", "input": {}},
                    {"type": "text", "text": "Done"}
                ]),
            ),
            make_user_msg(
                "u1",
                true,
                serde_json::json!([{
                    "type": "tool_result",
                    "tool_use_id": "tu_1",
                    "content": "file contents"
                }]),
                None,
            ),
        ];
        assert!(check_messages_ongoing(&messages));
    }

    #[test]
    fn test_exit_plan_mode_is_ending() {
        let messages = vec![
            make_assistant_msg(
                "a1",
                serde_json::json!([{"type": "text", "text": "Plan here"}]),
            ),
            make_assistant_msg(
                "a2",
                serde_json::json!([
                    {"type": "tool_use", "id": "tu_1", "name": "ExitPlanMode", "input": {}}
                ]),
            ),
        ];
        assert!(!check_messages_ongoing(&messages));
    }

    #[test]
    fn test_shutdown_response_is_ending() {
        let messages = vec![
            make_assistant_msg(
                "a1",
                serde_json::json!([
                    {"type": "tool_use", "id": "tu_1", "name": "SendMessage", "input": {
                        "type": "shutdown_response",
                        "request_id": "abc",
                        "approve": true
                    }}
                ]),
            ),
        ];
        assert!(!check_messages_ongoing(&messages));
    }

    #[test]
    fn test_shutdown_response_and_result_both_ending() {
        let messages = vec![
            make_assistant_msg(
                "a1",
                serde_json::json!([
                    {"type": "tool_use", "id": "tu_1", "name": "SendMessage", "input": {
                        "type": "shutdown_response",
                        "request_id": "abc",
                        "approve": true
                    }}
                ]),
            ),
            make_user_msg(
                "u1",
                true,
                serde_json::json!([{
                    "type": "tool_result",
                    "tool_use_id": "tu_1",
                    "content": "ok"
                }]),
                None,
            ),
        ];
        assert!(!check_messages_ongoing(&messages));
    }

    #[test]
    fn test_user_rejection_is_ending() {
        let messages = vec![
            make_assistant_msg(
                "a1",
                serde_json::json!([
                    {"type": "tool_use", "id": "tu_1", "name": "Write", "input": {}},
                    {"type": "text", "text": "Wrote file"}
                ]),
            ),
            make_user_msg(
                "u1",
                true,
                serde_json::json!([{
                    "type": "tool_result",
                    "tool_use_id": "tu_1",
                    "content": "rejected"
                }]),
                Some(serde_json::Value::String("User rejected tool use".to_string())),
            ),
        ];
        assert!(!check_messages_ongoing(&messages));
    }

    #[test]
    fn test_interruption_is_ending() {
        let messages = vec![
            make_assistant_msg(
                "a1",
                serde_json::json!([
                    {"type": "thinking", "thinking": "hmm", "signature": "sig"},
                    {"type": "text", "text": "partial"}
                ]),
            ),
            make_user_msg(
                "u1",
                false,
                serde_json::json!([{
                    "type": "text",
                    "text": "[Request interrupted by user"
                }]),
                None,
            ),
        ];
        assert!(!check_messages_ongoing(&messages));
    }

    #[test]
    fn test_activity_after_interruption_is_ongoing() {
        let messages = vec![
            make_assistant_msg(
                "a1",
                serde_json::json!([
                    {"type": "thinking", "thinking": "hmm", "signature": "sig"},
                    {"type": "text", "text": "partial"}
                ]),
            ),
            make_user_msg(
                "u1",
                false,
                serde_json::json!([{
                    "type": "text",
                    "text": "[Request interrupted by user"
                }]),
                None,
            ),
            make_assistant_msg(
                "a2",
                serde_json::json!([
                    {"type": "thinking", "thinking": "continuing...", "signature": "sig"}
                ]),
            ),
        ];
        assert!(check_messages_ongoing(&messages));
    }

    #[test]
    fn test_only_thinking_no_ending_is_ongoing() {
        let messages = vec![make_assistant_msg(
            "a1",
            serde_json::json!([{
                "type": "thinking",
                "thinking": "let me think about this",
                "signature": "sig"
            }]),
        )];
        assert!(check_messages_ongoing(&messages));
    }

    #[test]
    fn test_only_tool_use_no_ending_is_ongoing() {
        let messages = vec![make_assistant_msg(
            "a1",
            serde_json::json!([{
                "type": "tool_use",
                "id": "tu_1",
                "name": "Read",
                "input": {}
            }]),
        )];
        assert!(check_messages_ongoing(&messages));
    }

    #[test]
    fn test_send_message_non_shutdown_is_tool_use() {
        // 非 shutdown_response 类型的 SendMessage 应视为进行中（tool_use）
        let messages = vec![
            make_assistant_msg(
                "a1",
                serde_json::json!([{"type": "text", "text": "Sending message"}]),
            ),
            make_assistant_msg(
                "a2",
                serde_json::json!([
                    {"type": "tool_use", "id": "tu_1", "name": "SendMessage", "input": {
                        "to": "team-lead",
                        "message": "hello"
                    }}
                ]),
            ),
        ];
        assert!(check_messages_ongoing(&messages));
    }

    #[test]
    fn test_shutdown_reject_is_ongoing() {
        // approve=false 的 shutdown_response 应视为进行中
        let messages = vec![
            make_assistant_msg(
                "a1",
                serde_json::json!([{"type": "text", "text": "Not shutting down"}]),
            ),
            make_assistant_msg(
                "a2",
                serde_json::json!([
                    {"type": "tool_use", "id": "tu_1", "name": "SendMessage", "input": {
                        "type": "shutdown_response",
                        "request_id": "abc",
                        "approve": false
                    }}
                ]),
            ),
        ];
        assert!(check_messages_ongoing(&messages));
    }
}
