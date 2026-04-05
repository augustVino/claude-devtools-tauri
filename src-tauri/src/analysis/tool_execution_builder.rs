//! ToolExecutionBuilder - Builds tool execution tracking from messages.
//!
//! Matches tool calls with their results using a two-pass algorithm:
//! 1. sourceToolUseID for accurate internal user message matching
//! 2. toolResults array fallback for other patterns
//!
//! Aligned with the Electron implementation at
//! `src/main/services/analysis/ToolExecutionBuilder.ts`.

use std::collections::{HashMap, HashSet};

use crate::types::chunks::ToolExecution;
#[allow(unused_imports)]
use crate::types::messages::{ParsedMessage, ToolCall, ToolResult};
use crate::utils::timestamp::parse_ts_ms;

/// Build tool execution tracking from parsed messages.
///
/// Uses a three-pass algorithm:
/// - **Pass 1**: Collect all tool calls into a HashMap (id -> {call, startTime})
/// - **Pass 2**: Match tool results to calls, first via `sourceToolUseID`,
///   then via the `toolResults` array. Unmatched calls are added as pending.
/// - **Pass 3**: Sort all executions by start time.
pub fn build_tool_executions(messages: &[ParsedMessage]) -> Vec<ToolExecution> {
    // Pass 1: collect all tool calls keyed by their id
    let tool_call_map: HashMap<String, (&ToolCall, u64)> = messages
        .iter()
        .flat_map(|msg| {
            let ts_ms = parse_ts_ms(&msg.timestamp);
            msg.tool_calls
                .iter()
                .map(move |tc| (tc.id.clone(), (tc, ts_ms)))
        })
        .collect();

    let mut executions: Vec<ToolExecution> = Vec::new();
    let mut matched_result_ids: HashSet<String> = HashSet::new();
    let mut matched_call_ids: HashSet<String> = HashSet::new();

    // Pass 2: match results to tool calls
    for msg in messages {
        // Strategy A: use sourceToolUseID (most accurate for internal user messages)
        if let Some(ref source_id) = msg.source_tool_use_id {
            if let Some(&(call, start_time)) = tool_call_map.get(source_id) {
                if !msg.tool_results.is_empty() {
                    let result = &msg.tool_results[0];
                    matched_result_ids.insert(result.tool_use_id.clone());
                    matched_call_ids.insert(source_id.clone());

                    let end_time = parse_ts_ms(&msg.timestamp);
                    let duration_ms = compute_duration(start_time, end_time);

                    executions.push(ToolExecution {
                        tool_call: call.clone(),
                        result: Some(result.clone()),
                        start_time,
                        end_time: Some(end_time),
                        duration_ms,
                    });
                }
            }
        }

        // Strategy B: iterate toolResults for any results not matched above
        for result in &msg.tool_results {
            // Skip results already matched via sourceToolUseID
            if matched_result_ids.contains(&result.tool_use_id) {
                continue;
            }

            if let Some(&(call, start_time)) = tool_call_map.get(&result.tool_use_id) {
                matched_call_ids.insert(result.tool_use_id.clone());

                let end_time = parse_ts_ms(&msg.timestamp);
                let duration_ms = compute_duration(start_time, end_time);

                executions.push(ToolExecution {
                    tool_call: call.clone(),
                    result: Some(result.clone()),
                    start_time,
                    end_time: Some(end_time),
                    duration_ms,
                });
            }
        }
    }

    // Add tool calls that have no results as pending executions
    for (id, (call, start_time)) in &tool_call_map {
        if !matched_call_ids.contains(id) {
            executions.push(ToolExecution {
                tool_call: (*call).clone(),
                result: None,
                start_time: *start_time,
                end_time: None,
                duration_ms: None,
            });
        }
    }

    // Pass 3: sort by start time
    executions.sort_by(|a, b| a.start_time.cmp(&b.start_time));

    executions
}

/// Compute the duration between two u64 timestamps, returning `Some(duration)`
/// when `end > start`, or `None` otherwise.
pub fn compute_duration(start: u64, end: u64) -> Option<u64> {
    if end > start {
        Some(end.saturating_sub(start))
    } else {
        None
    }
}

/// Parse the duration in milliseconds between two RFC 3339 timestamp strings.
///
/// Returns `None` if either timestamp cannot be parsed or if the duration is zero.
#[cfg(test)]
fn parse_duration_ms(start: &str, end: &str) -> Option<u64> {
    let start_dt = chrono::DateTime::parse_from_rfc3339(start).ok()?;
    let end_dt = chrono::DateTime::parse_from_rfc3339(end).ok()?;
    let duration = end_dt.timestamp_millis().saturating_sub(start_dt.timestamp_millis()) as u64;
    if duration > 0 { Some(duration) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::domain::MessageType;

    fn make_tool_call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            input: serde_json::Value::Object(serde_json::Map::new()),
            is_task: false,
            task_description: None,
            task_subagent_type: None,
        }
    }

    fn make_tool_result(tool_use_id: &str) -> ToolResult {
        ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content: serde_json::Value::String("done".to_string()),
            is_error: false,
        }
    }

    fn make_msg(
        uuid: &str,
        ts: &str,
        message_type: MessageType,
        tool_calls: Vec<ToolCall>,
        tool_results: Vec<ToolResult>,
        source_tool_use_id: Option<String>,
    ) -> ParsedMessage {
        ParsedMessage {
            uuid: uuid.to_string(),
            parent_uuid: None,
            message_type,
            timestamp: ts.to_string(),
            role: Some("assistant".to_string()),
            content: serde_json::Value::String("msg".to_string()),
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
            source_tool_use_id,
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            is_compact_summary: None,
            request_id: None,
        }
    }

    #[test]
    fn test_empty_messages() {
        let execs = build_tool_executions(&[]);
        assert!(execs.is_empty());
    }

    #[test]
    fn test_call_without_result() {
        let msgs = vec![make_msg(
            "a1",
            "2026-01-01T00:00:00Z",
            MessageType::Assistant,
            vec![make_tool_call("tc1", "Read")],
            vec![],
            None,
        )];

        let execs = build_tool_executions(&msgs);
        assert_eq!(execs.len(), 1);
        assert_eq!(execs[0].tool_call.id, "tc1");
        assert!(execs[0].result.is_none());
        assert!(execs[0].end_time.is_none());
        assert!(execs[0].duration_ms.is_none());
    }

    #[test]
    fn test_call_matched_via_tool_results() {
        let msgs = vec![
            make_msg(
                "a1",
                "2026-01-01T00:00:00Z",
                MessageType::Assistant,
                vec![make_tool_call("tc1", "Read")],
                vec![],
                None,
            ),
            make_msg(
                "u1",
                "2026-01-01T00:00:02Z",
                MessageType::User,
                vec![],
                vec![make_tool_result("tc1")],
                None,
            ),
        ];

        let execs = build_tool_executions(&msgs);
        assert_eq!(execs.len(), 1);
        assert_eq!(execs[0].tool_call.id, "tc1");
        assert!(execs[0].result.is_some());
        assert_eq!(execs[0].end_time, Some(parse_ts_ms("2026-01-01T00:00:02Z")));
        assert_eq!(execs[0].duration_ms, Some(2000));
    }

    #[test]
    fn test_call_matched_via_source_tool_use_id() {
        let msgs = vec![
            make_msg(
                "a1",
                "2026-01-01T00:00:00Z",
                MessageType::Assistant,
                vec![make_tool_call("tc1", "Bash")],
                vec![],
                None,
            ),
            make_msg(
                "u1",
                "2026-01-01T00:00:05Z",
                MessageType::User,
                vec![],
                vec![make_tool_result("tc1")],
                Some("tc1".to_string()),
            ),
        ];

        let execs = build_tool_executions(&msgs);
        assert_eq!(execs.len(), 1);
        assert_eq!(execs[0].tool_call.id, "tc1");
        assert!(execs[0].result.is_some());
        assert_eq!(execs[0].duration_ms, Some(5000));
    }

    #[test]
    fn test_source_tool_use_id_prevents_double_match() {
        // When a result is matched via sourceToolUseID, it should not be
        // matched again in the toolResults loop.
        let msgs = vec![
            make_msg(
                "a1",
                "2026-01-01T00:00:00Z",
                MessageType::Assistant,
                vec![make_tool_call("tc1", "Read")],
                vec![],
                None,
            ),
            make_msg(
                "u1",
                "2026-01-01T00:00:03Z",
                MessageType::User,
                vec![],
                vec![make_tool_result("tc1")],
                Some("tc1".to_string()),
            ),
        ];

        let execs = build_tool_executions(&msgs);
        assert_eq!(execs.len(), 1);
    }

    #[test]
    fn test_mixed_matched_and_pending_calls() {
        let msgs = vec![
            make_msg(
                "a1",
                "2026-01-01T00:00:00Z",
                MessageType::Assistant,
                vec![
                    make_tool_call("tc1", "Read"),
                    make_tool_call("tc2", "Write"),
                    make_tool_call("tc3", "Bash"),
                ],
                vec![],
                None,
            ),
            make_msg(
                "u1",
                "2026-01-01T00:00:02Z",
                MessageType::User,
                vec![],
                vec![make_tool_result("tc1")],
                None,
            ),
        ];

        let execs = build_tool_executions(&msgs);
        assert_eq!(execs.len(), 3);

        // tc1 should be matched, tc2 and tc3 should be pending
        let matched: Vec<_> = execs.iter().filter(|e| e.result.is_some()).collect();
        let pending: Vec<_> = execs.iter().filter(|e| e.result.is_none()).collect();
        assert_eq!(matched.len(), 1);
        assert_eq!(pending.len(), 2);
    }

    #[test]
    fn test_sorted_by_start_time() {
        let msgs = vec![
            make_msg(
                "a2",
                "2026-01-01T00:00:10Z",
                MessageType::Assistant,
                vec![make_tool_call("tc2", "Write")],
                vec![],
                None,
            ),
            make_msg(
                "a1",
                "2026-01-01T00:00:00Z",
                MessageType::Assistant,
                vec![make_tool_call("tc1", "Read")],
                vec![],
                None,
            ),
        ];

        let execs = build_tool_executions(&msgs);
        assert_eq!(execs.len(), 2);
        assert_eq!(execs[0].tool_call.id, "tc1");
        assert_eq!(execs[1].tool_call.id, "tc2");
    }

    #[test]
    fn test_parse_duration_ms_valid() {
        let start = "2026-01-01T00:00:00Z";
        let end = "2026-01-01T00:00:05Z";
        assert_eq!(parse_duration_ms(start, end), Some(5000));
    }

    #[test]
    fn test_parse_duration_ms_invalid_start() {
        let start = "not-a-timestamp";
        let end = "2026-01-01T00:00:05Z";
        assert_eq!(parse_duration_ms(start, end), None);
    }

    #[test]
    fn test_parse_duration_ms_invalid_end() {
        let start = "2026-01-01T00:00:00Z";
        let end = "not-a-timestamp";
        assert_eq!(parse_duration_ms(start, end), None);
    }
}
