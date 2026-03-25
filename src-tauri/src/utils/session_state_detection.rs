//! Session state detection utilities for determining if sessions are ongoing.
//!
//! A session is considered "ongoing" if there are AI-related activities
//! (thinking, tool_use, tool_result) AFTER the last "ending" event.
//!
//! Ending events: text_output, interruption, exit_plan_mode.
//! Ongoing indicators: thinking, tool_use, tool_result.

use crate::types::domain::MessageType;
use crate::types::messages::ParsedMessage;

/// Activity types for tracking session state.
#[derive(Debug, Clone, PartialEq)]
enum ActivityType {
    TextOutput,
    Thinking,
    ToolUse,
    ToolResult,
    Interruption,
    ExitPlanMode,
}

/// Activity entry with type and order index.
struct Activity {
    kind: ActivityType,
    index: usize,
}

/// Check if a toolUseResult value indicates a user-rejected tool use.
fn is_tool_use_rejection(tool_use_result: &serde_json::Value) -> bool {
    tool_use_result.as_str() == Some("User rejected tool use")
}

/// Check if a tool_use block is a SendMessage shutdown_response with approve: true.
fn is_shutdown_response(name: &str, input: &serde_json::Value) -> bool {
    name == "SendMessage"
        && input.get("type").and_then(|v| v.as_str()) == Some("shutdown_response")
        && input.get("approve").and_then(|v| v.as_bool()) == Some(true)
}

/// Check if activities indicate an ongoing session.
///
/// Session is ongoing if any AI activity (thinking, tool_use, tool_result)
/// appears AFTER the last ending event (text_output, interruption, exit_plan_mode).
fn is_ongoing_from_activities(activities: &[Activity]) -> bool {
    if activities.is_empty() {
        return false;
    }

    // Find the index of the last "ending" event
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
            // No ending event found -- ongoing if there is any AI activity at all
            activities.iter().any(|a| {
                matches!(
                    a.kind,
                    ActivityType::Thinking | ActivityType::ToolUse | ActivityType::ToolResult
                )
            })
        }
        Some(ending_idx) => {
            // Check if there are any AI activities AFTER the last ending event
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

/// Check if messages indicate an ongoing session (AI response in progress).
///
/// This mirrors the Electron app's `checkMessagesOngoing` logic:
/// - Tracks ending events and ongoing indicators via sequential index.
/// - Special handling for ExitPlanMode (treated as ending) and
///   SendMessage shutdown_response (both the call and its result are endings).
/// - User-rejected tool uses are treated as interruptions (endings).
/// - "[Request interrupted by user" messages are interruptions (endings).
pub fn check_messages_ongoing(messages: &[ParsedMessage]) -> bool {
    let mut activities: Vec<Activity> = Vec::new();
    let mut activity_index: usize = 0;
    // Track tool_use IDs that are shutdown responses so their tool_results are also ending events
    let mut shutdown_tool_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    for msg in messages {
        if msg.message_type == MessageType::Assistant {
            // Process assistant message content blocks
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
                                // ExitPlanMode is a special ending tool
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
            // Check if this is a user-rejected tool use (ending event)
            let is_rejection = msg
                .tool_use_result
                .as_ref()
                .map(is_tool_use_rejection)
                .unwrap_or(false);

            // Check for tool results and interruptions in user messages
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
                                // Shutdown tool result = ending event
                                activities.push(Activity {
                                    kind: ActivityType::Interruption,
                                    index: activity_index,
                                });
                                activity_index += 1;
                            } else if is_rejection {
                                // User rejection = ending event
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
        // SendMessage without shutdown_response should be treated as ongoing (tool_use)
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
        // SendMessage shutdown_response with approve=false should be treated as ongoing
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
