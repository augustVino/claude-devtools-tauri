//! ConversationGroupBuilder - Alternative grouping strategy for conversation flow.
//!
//! Groups one user message with all AI responses until the next user message.
//! This is a cleaner alternative to buildChunks() that:
//! - Uses simpler time-based grouping
//! - Separates Task executions from regular tool executions
//! - Links subagents more explicitly via TaskExecution
//!
//! Aligned with the Electron implementation at
//! `src/main/services/analysis/ConversationGroupBuilder.ts`.

use std::collections::HashMap;

use crate::parsing::{calculate_metrics, is_real_user_message};
use crate::types::chunks::{ConversationGroup, Process, TaskExecution, ToolExecution};
use crate::types::domain::MessageType;
use crate::types::messages::ParsedMessage;

/// Build conversation groups using simplified grouping strategy.
///
/// Groups one user message with all AI responses until the next user message.
pub fn build_groups(messages: &[ParsedMessage], subagents: &[Process]) -> Vec<ConversationGroup> {
    let mut groups: Vec<ConversationGroup> = Vec::new();

    // Step 1: Filter to main thread only (not sidechain)
    let main_messages: Vec<&ParsedMessage> = messages.iter().filter(|m| !m.is_sidechain).collect();

    // Step 2: Find all REAL user messages (these start groups)
    let user_messages: Vec<&ParsedMessage> = main_messages
        .iter()
        .filter(|m| is_real_user_message(m))
        .copied()
        .collect();

    // Step 3: For each user message, collect all AI responses until next user message
    for (i, user_msg) in user_messages.iter().enumerate() {
        let next_user_msg = user_messages.get(i + 1).copied();

        // Collect all messages between this user message and the next
        let ai_responses = collect_ai_responses(&main_messages, user_msg, next_user_msg);

        // Separate Task tool results from regular tool executions
        let (task_executions, regular_tool_executions) =
            separate_task_executions(&ai_responses, subagents);

        // Link subagents to this group
        let group_subagents = link_subagents_to_group(user_msg, next_user_msg, subagents);

        // Calculate timing
        let (start_time, end_time, duration_ms) =
            calculate_group_timing(user_msg, &ai_responses);

        // Calculate metrics
        let mut all_group_messages: Vec<ParsedMessage> = vec![(*user_msg).clone()];
        all_group_messages.extend(ai_responses.iter().cloned());
        let metrics = calculate_metrics(&all_group_messages);

        groups.push(ConversationGroup {
            id: format!("group-{}", i + 1),
            r#type: "user-ai-exchange".to_string(),
            user_message: (*user_msg).clone(),
            ai_responses,
            processes: group_subagents,
            tool_executions: regular_tool_executions,
            task_executions,
            start_time,
            end_time,
            duration_ms,
            metrics,
        });
    }

    groups
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Collect AI responses between a user message and the next user message.
///
/// Includes assistant messages and internal (meta) user messages within
/// the time window between two consecutive real user messages.
fn collect_ai_responses(
    messages: &[&ParsedMessage],
    user_msg: &ParsedMessage,
    next_user_msg: Option<&ParsedMessage>,
) -> Vec<ParsedMessage> {
    let mut responses: Vec<ParsedMessage> = Vec::new();

    let start_ts = &user_msg.timestamp;
    let end_ts = next_user_msg.map(|m| m.timestamp.clone());

    for msg in messages {
        // Skip if before or at this user message
        if msg.timestamp <= *start_ts {
            continue;
        }

        // Skip if at or after next user message
        if let Some(ref end) = end_ts {
            if msg.timestamp >= *end {
                continue;
            }
        }

        // Include assistant messages and internal user messages
        if msg.message_type == MessageType::Assistant
            || (msg.message_type == MessageType::User && msg.is_meta)
        {
            responses.push((*msg).clone());
        }
    }

    responses
}

/// Separate Task executions from regular tool executions.
///
/// Task tools spawn subagents, so we track them separately to avoid duplication.
///
/// **Known limitation:** `SubagentResolver::parse_subagent_file()` always sets
/// `task_id: None`, so after `From` conversion, `chunks::Process.parent_task_id`
/// is always `None`. This means `subagent_map` is always empty, and
/// `task_executions` will always be empty. All Task tool calls fall through to
/// `regular_tool_executions`.
fn separate_task_executions(
    responses: &[ParsedMessage],
    all_subagents: &[Process],
) -> (Vec<TaskExecution>, Vec<ToolExecution>) {
    let mut task_executions: Vec<TaskExecution> = Vec::new();
    let mut regular_tool_executions: Vec<ToolExecution> = Vec::new();

    // Build map of tool_call_id -> subagent for Task calls
    // (parent_task_id maps the tool call ID to the subagent)
    let subagent_map: HashMap<&str, &Process> = all_subagents
        .iter()
        .filter_map(|s| s.parent_task_id.as_deref().map(|tid| (tid, s)))
        .collect();

    // Collect all tool calls: id -> (call, timestamp)
    let tool_call_map: HashMap<&str, (&crate::types::messages::ToolCall, &str)> = responses
        .iter()
        .filter(|m| m.message_type == MessageType::Assistant)
        .flat_map(|msg| {
            msg.tool_calls
                .iter()
                .map(|tc| (tc.id.as_str(), (tc, msg.timestamp.as_str())))
        })
        .collect();

    // Match with results (internal user messages with source_tool_use_id)
    for msg in responses {
        if msg.message_type != MessageType::User || !msg.is_meta {
            continue;
        }

        let source_id = match &msg.source_tool_use_id {
            Some(id) => id.as_str(),
            None => continue,
        };

        let (call, call_ts) = match tool_call_map.get(source_id) {
            Some(info) => *info,
            None => continue,
        };

        // Check if this is a Task call with a matching subagent
        let subagent = subagent_map.get(source_id);
        if call.name == "Task" {
            if let Some(sub) = subagent {
                let call_ts_ms = parse_ts_ms(call_ts);
                let result_ts_ms = parse_ts_ms(&msg.timestamp);
                let duration_ms = if result_ts_ms > call_ts_ms {
                    (result_ts_ms - call_ts_ms) as u64
                } else {
                    0
                };

                task_executions.push(TaskExecution {
                    task_id: call.id.clone(),
                    tool_call_id: call.id.clone(),
                    input: call.input.clone(),
                    subagent: (*sub).clone(),
                    tool_result: msg.clone(),
                    task_call_timestamp: call_ts_ms,
                    result_timestamp: result_ts_ms,
                    duration_ms,
                });
                continue;
            }
        }

        // Regular tool execution
        let result = msg.tool_results.first();
        if let Some(r) = result {
            let end_time = Some(msg.timestamp.clone());
            let duration_ms = parse_duration_ms(call_ts, &msg.timestamp);

            regular_tool_executions.push(ToolExecution {
                tool_call: (*call).clone(),
                result: Some(r.clone()),
                start_time: call_ts.to_string(),
                end_time,
                duration_ms,
            });
        }
    }

    (task_executions, regular_tool_executions)
}

/// Link subagents to a conversation group based on timing.
///
/// A subagent belongs to a group if its start_time falls within the group's
/// time range (from user message timestamp to next user message timestamp).
fn link_subagents_to_group(
    user_msg: &ParsedMessage,
    next_user_msg: Option<&ParsedMessage>,
    all_subagents: &[Process],
) -> Vec<Process> {
    let group_start = parse_ts_ms(&user_msg.timestamp);
    // If no next user message, use a far-future timestamp so all subagents match
    let group_end = next_user_msg
        .map(|m| parse_ts_ms(&m.timestamp))
        .unwrap_or(f64::MAX);

    all_subagents
        .iter()
        .filter(|s| {
            let s_start = s.start_time as f64;
            s_start >= group_start && s_start < group_end
        })
        .cloned()
        .collect()
}

/// Calculate group timing from user message and AI responses.
///
/// Returns (start_time_ms, end_time_ms, duration_ms).
fn calculate_group_timing(
    user_msg: &ParsedMessage,
    ai_responses: &[ParsedMessage],
) -> (f64, f64, u64) {
    let start_time = parse_ts_ms(&user_msg.timestamp);

    let mut end_time = start_time;
    for resp in ai_responses {
        let ts = parse_ts_ms(&resp.timestamp);
        if ts > end_time {
            end_time = ts;
        }
    }

    let duration_ms = if end_time > start_time {
        (end_time - start_time) as u64
    } else {
        0
    };

    (start_time, end_time, duration_ms)
}

// =============================================================================
// Timestamp Helpers
// =============================================================================

/// Parse an RFC 3339 timestamp string to epoch milliseconds as f64.
///
/// Returns 0.0 if the timestamp cannot be parsed.
fn parse_ts_ms(ts: &str) -> f64 {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|dt| dt.timestamp_millis() as f64)
        .unwrap_or(0.0)
}

/// Parse the duration in milliseconds between two RFC 3339 timestamp strings.
///
/// Returns `None` if either timestamp cannot be parsed.
fn parse_duration_ms(start: &str, end: &str) -> Option<u64> {
    let start_dt = chrono::DateTime::parse_from_rfc3339(start).ok()?;
    let end_dt = chrono::DateTime::parse_from_rfc3339(end).ok()?;
    Some(end_dt.timestamp_millis().saturating_sub(start_dt.timestamp_millis()) as u64)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::domain::MessageType;

    fn make_user_msg(uuid: &str, ts: &str, is_meta: bool) -> ParsedMessage {
        ParsedMessage {
            uuid: uuid.to_string(),
            parent_uuid: None,
            message_type: MessageType::User,
            timestamp: ts.to_string(),
            role: Some("user".to_string()),
            content: serde_json::Value::String("hello".to_string()),
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
            tool_use_result: None,
            is_compact_summary: None,
            request_id: None,
        }
    }

    fn make_assistant_msg(uuid: &str, ts: &str) -> ParsedMessage {
        ParsedMessage {
            uuid: uuid.to_string(),
            parent_uuid: None,
            message_type: MessageType::Assistant,
            timestamp: ts.to_string(),
            role: Some("assistant".to_string()),
            content: serde_json::Value::String("response".to_string()),
            usage: None,
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

    fn make_meta_user_msg(uuid: &str, ts: &str) -> ParsedMessage {
        ParsedMessage {
            uuid: uuid.to_string(),
            parent_uuid: None,
            message_type: MessageType::User,
            timestamp: ts.to_string(),
            role: Some("user".to_string()),
            content: serde_json::Value::String("tool result".to_string()),
            usage: None,
            model: None,
            cwd: None,
            git_branch: None,
            agent_id: None,
            is_sidechain: false,
            is_meta: true,
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

    // --- Test 1: Empty messages produce no groups ---

    #[test]
    fn test_empty_messages() {
        let groups = build_groups(&[], &[]);
        assert!(groups.is_empty());
    }

    // --- Test 2: Single user message produces one group ---

    #[test]
    fn test_single_user_message() {
        let messages = vec![make_user_msg("u1", "2026-01-01T00:00:00Z", false)];

        let groups = build_groups(&messages, &[]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, "group-1");
        assert_eq!(groups[0].r#type, "user-ai-exchange");
        assert_eq!(groups[0].user_message.uuid, "u1");
        assert!(groups[0].ai_responses.is_empty());
        assert!(groups[0].tool_executions.is_empty());
        assert!(groups[0].task_executions.is_empty());
        assert!(groups[0].processes.is_empty());
    }

    // --- Test 3: User-AI exchange produces correct grouping ---

    #[test]
    fn test_user_ai_exchange() {
        let messages = vec![
            make_user_msg("u1", "2026-01-01T00:00:00Z", false),
            make_assistant_msg("a1", "2026-01-01T00:00:01Z"),
            make_meta_user_msg("m1", "2026-01-01T00:00:02Z"),
            make_assistant_msg("a2", "2026-01-01T00:00:03Z"),
        ];

        let groups = build_groups(&messages, &[]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].user_message.uuid, "u1");

        // AI responses: 2 assistant messages + 1 meta user message
        assert_eq!(groups[0].ai_responses.len(), 3);
        assert_eq!(groups[0].ai_responses[0].uuid, "a1");
        assert_eq!(groups[0].ai_responses[1].uuid, "m1");
        assert_eq!(groups[0].ai_responses[2].uuid, "a2");

        // Timing: start at user message, end at last AI response
        assert!(groups[0].start_time > 0.0);
        assert!(groups[0].end_time > groups[0].start_time);
        assert!(groups[0].duration_ms > 0);
    }

    // --- Test 4: Two groups from two user messages ---

    #[test]
    fn test_two_groups() {
        let messages = vec![
            make_user_msg("u1", "2026-01-01T00:00:00Z", false),
            make_assistant_msg("a1", "2026-01-01T00:00:01Z"),
            make_user_msg("u2", "2026-01-01T00:00:05Z", false),
            make_assistant_msg("a2", "2026-01-01T00:00:06Z"),
            make_assistant_msg("a3", "2026-01-01T00:00:07Z"),
        ];

        let groups = build_groups(&messages, &[]);
        assert_eq!(groups.len(), 2);

        // First group
        assert_eq!(groups[0].id, "group-1");
        assert_eq!(groups[0].user_message.uuid, "u1");
        assert_eq!(groups[0].ai_responses.len(), 1);
        assert_eq!(groups[0].ai_responses[0].uuid, "a1");

        // Second group
        assert_eq!(groups[1].id, "group-2");
        assert_eq!(groups[1].user_message.uuid, "u2");
        assert_eq!(groups[1].ai_responses.len(), 2);
        assert_eq!(groups[1].ai_responses[0].uuid, "a2");
        assert_eq!(groups[1].ai_responses[1].uuid, "a3");

        // Verify timing isolation: first group ends before second group starts
        assert!(groups[0].end_time <= groups[1].start_time);
    }
}
