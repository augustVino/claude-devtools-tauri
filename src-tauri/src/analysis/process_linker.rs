use std::collections::HashSet;

use crate::types::chunks::{AiChunk, Process};

/// Link subagent processes to an AI chunk using a two-tier strategy.
///
/// 1. **Primary:** Match subagents whose `parent_task_id` corresponds to a Task tool call
///    within this chunk's responses. This is reliable even when the response is in progress.
/// 2. **Fallback:** For orphaned subagents (no `parent_task_id`), check whether the
///    subagent's `start_time` falls within the chunk's `[start_time, end_time]` range.
pub fn link_processes_to_chunk(chunk: &mut AiChunk, subagents: &[Process]) {
    let task_ids = collect_task_ids(&chunk.responses);
    let mut linked_ids: HashSet<String> = HashSet::new();

    // Primary linking: parent_task_id matching
    for subagent in subagents {
        if let Some(ref parent_id) = subagent.parent_task_id {
            if task_ids.contains(parent_id) {
                chunk.processes.push(subagent.clone());
                linked_ids.insert(subagent.id.clone());
            }
        }
    }

    // Fallback linking: timing-based for orphaned subagents
    for subagent in subagents {
        if linked_ids.contains(&subagent.id) {
            continue;
        }

        // Only apply timing fallback when the subagent has no parent_task_id at all.
        // If it has a parent_task_id that didn't match, it belongs to a different chunk.
        if subagent.parent_task_id.is_none() {
            if subagent.start_time >= chunk.start_time && subagent.start_time <= chunk.end_time {
                chunk.processes.push(subagent.clone());
            }
        }
    }

    // Sort linked processes by start_time for deterministic ordering
    chunk
        .processes
        .sort_by_key(|p| p.start_time);
}

/// Collect all Task tool call IDs from a slice of assistant messages.
fn collect_task_ids(responses: &[crate::types::messages::ParsedMessage]) -> HashSet<String> {
    let mut ids = HashSet::new();
    for response in responses {
        for tool_call in &response.tool_calls {
            if tool_call.is_task {
                ids.insert(tool_call.id.clone());
            }
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::chunks::AiChunk;
    use crate::types::domain::SessionMetrics;
    use crate::types::messages::ParsedMessage;

    fn make_chunk(
        start_time: u64,
        end_time: u64,
        responses: Vec<ParsedMessage>,
    ) -> AiChunk {
        AiChunk {
            id: "test-chunk".to_string(),
            start_time,
            end_time,
            duration_ms: end_time.saturating_sub(start_time),
            metrics: SessionMetrics::default(),
            responses,
            processes: vec![],
            sidechain_messages: vec![],
            tool_executions: vec![],
            semantic_steps: vec![],
            semantic_step_groups: vec![],
        }
    }

    fn make_subagent(
        id: &str,
        start_time: u64,
        end_time: u64,
        parent_task_id: Option<&str>,
    ) -> Process {
        Process {
            id: id.to_string(),
            file_path: format!("/path/to/{}", id),
            description: None,
            subagent_type: None,
            messages: vec![],
            start_time,
            end_time,
            duration_ms: end_time.saturating_sub(start_time),
            metrics: SessionMetrics::default(),
            is_parallel: false,
            parent_task_id: parent_task_id.map(String::from),
            is_ongoing: None,
            main_session_impact: None,
            team: None,
        }
    }

    fn make_response_with_task(task_id: &str) -> ParsedMessage {
        ParsedMessage {
            uuid: "msg-1".to_string(),
            parent_uuid: None,
            message_type: crate::types::domain::MessageType::Assistant,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            role: Some("assistant".to_string()),
            content: serde_json::Value::String("text".to_string()),
            usage: None,
            model: None,
            cwd: None,
            git_branch: None,
            agent_id: None,
            is_sidechain: false,
            is_meta: false,
            user_type: None,
            tool_calls: vec![crate::types::messages::ToolCall {
                id: task_id.to_string(),
                name: "Task".to_string(),
                input: serde_json::Value::Object(serde_json::Map::new()),
                is_task: true,
                task_description: None,
                task_subagent_type: None,
            }],
            tool_results: vec![],
            source_tool_use_id: None,
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            is_compact_summary: None,
            request_id: None,
        }
    }

    fn make_response_without_tasks() -> ParsedMessage {
        ParsedMessage {
            uuid: "msg-2".to_string(),
            parent_uuid: None,
            message_type: crate::types::domain::MessageType::Assistant,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            role: Some("assistant".to_string()),
            content: serde_json::Value::String("text".to_string()),
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
        }
    }

    #[test]
    fn primary_linking_matches_by_parent_task_id() {
        let response = make_response_with_task("task-1");
        let mut chunk = make_chunk(1000, 2000, vec![response]);

        let subagents = vec![
            make_subagent("sub-1", 1100, 1800, Some("task-1")),
            make_subagent("sub-2", 1200, 1900, Some("task-2")),
        ];

        link_processes_to_chunk(&mut chunk, &subagents);

        assert_eq!(chunk.processes.len(), 1);
        assert_eq!(chunk.processes[0].id, "sub-1");
    }

    #[test]
    fn fallback_linking_matches_orphaned_by_timing() {
        let response = make_response_without_tasks();
        let mut chunk = make_chunk(1000, 2000, vec![response]);

        let subagents = vec![
            make_subagent("orphan-1", 1100, 1800, None),
            make_subagent("orphan-2", 500, 900, None), // outside chunk range
        ];

        link_processes_to_chunk(&mut chunk, &subagents);

        assert_eq!(chunk.processes.len(), 1);
        assert_eq!(chunk.processes[0].id, "orphan-1");
    }

    #[test]
    fn subagent_with_unmatched_parent_task_id_is_not_fallback_linked() {
        let response = make_response_without_tasks();
        let mut chunk = make_chunk(1000, 2000, vec![response]);

        // Has a parent_task_id pointing to "task-other" which is NOT in this chunk,
        // so it should NOT be fallback-linked even though timing matches
        let subagents = vec![make_subagent("sub-1", 1100, 1800, Some("task-other"))];

        link_processes_to_chunk(&mut chunk, &subagents);

        assert!(chunk.processes.is_empty());
    }

    #[test]
    fn processes_are_sorted_by_start_time() {
        let response = make_response_with_task("task-1");
        let mut chunk = make_chunk(1000, 5000, vec![response]);

        let subagents = vec![
            make_subagent("sub-2", 3000, 4000, Some("task-2")),
            make_subagent("sub-1", 2000, 3500, Some("task-1")),
        ];

        link_processes_to_chunk(&mut chunk, &subagents);

        assert_eq!(chunk.processes.len(), 1);
        assert_eq!(chunk.processes[0].id, "sub-1");
    }

    #[test]
    fn no_linking_when_no_responses() {
        let mut chunk = make_chunk(1000, 2000, vec![]);
        let subagents = vec![make_subagent("sub-1", 1100, 1800, Some("task-1"))];

        link_processes_to_chunk(&mut chunk, &subagents);

        assert!(chunk.processes.is_empty());
    }

    #[test]
    fn no_linking_when_no_subagents() {
        let response = make_response_with_task("task-1");
        let mut chunk = make_chunk(1000, 2000, vec![response]);

        link_processes_to_chunk(&mut chunk, &[]);

        assert!(chunk.processes.is_empty());
    }

    #[test]
    fn timing_fallback_inclusive_boundary() {
        let response = make_response_without_tasks();
        let mut chunk = make_chunk(1000, 2000, vec![response]);

        // start_time exactly at chunk boundary
        let subagents = vec![make_subagent("boundary", 1000, 1500, None)];

        link_processes_to_chunk(&mut chunk, &subagents);

        assert_eq!(chunk.processes.len(), 1);
        assert_eq!(chunk.processes[0].id, "boundary");
    }

    #[test]
    fn timing_fallback_end_boundary() {
        let response = make_response_without_tasks();
        let mut chunk = make_chunk(1000, 2000, vec![response]);

        // start_time exactly at chunk end boundary
        let subagents = vec![make_subagent("end-boundary", 2000, 2500, None)];

        link_processes_to_chunk(&mut chunk, &subagents);

        assert_eq!(chunk.processes.len(), 1);
        assert_eq!(chunk.processes[0].id, "end-boundary");
    }

    #[test]
    fn primary_and_fallback_combined() {
        let response = make_response_with_task("task-1");
        let mut chunk = make_chunk(1000, 5000, vec![response]);

        let subagents = vec![
            make_subagent("primary-sub", 2000, 4000, Some("task-1")),
            make_subagent("fallback-sub", 3000, 4500, None),
            make_subagent("unrelated-sub", 800, 900, Some("task-other")),
        ];

        link_processes_to_chunk(&mut chunk, &subagents);

        assert_eq!(chunk.processes.len(), 2);
        assert_eq!(chunk.processes[0].id, "primary-sub");
        assert_eq!(chunk.processes[1].id, "fallback-sub");
    }

    #[test]
    fn duplicate_prevention_already_linked_not_fallback_linked() {
        let response = make_response_with_task("task-1");
        let mut chunk = make_chunk(1000, 5000, vec![response]);

        let subagents = vec![make_subagent("sub-1", 2000, 4000, Some("task-1"))];

        link_processes_to_chunk(&mut chunk, &subagents);

        // Already linked via primary, should not be double-linked
        assert_eq!(chunk.processes.len(), 1);
    }

    #[test]
    fn multiple_task_ids_in_response() {
        let response = ParsedMessage {
            uuid: "msg-multi".to_string(),
            parent_uuid: None,
            message_type: crate::types::domain::MessageType::Assistant,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            role: Some("assistant".to_string()),
            content: serde_json::Value::String("text".to_string()),
            usage: None,
            model: None,
            cwd: None,
            git_branch: None,
            agent_id: None,
            is_sidechain: false,
            is_meta: false,
            user_type: None,
            tool_calls: vec![
                crate::types::messages::ToolCall {
                    id: "task-a".to_string(),
                    name: "Task".to_string(),
                    input: serde_json::Value::Object(serde_json::Map::new()),
                    is_task: true,
                    task_description: None,
                    task_subagent_type: None,
                },
                crate::types::messages::ToolCall {
                    id: "task-b".to_string(),
                    name: "Read".to_string(), // not a task
                    input: serde_json::Value::Object(serde_json::Map::new()),
                    is_task: false,
                    task_description: None,
                    task_subagent_type: None,
                },
                crate::types::messages::ToolCall {
                    id: "task-c".to_string(),
                    name: "Task".to_string(),
                    input: serde_json::Value::Object(serde_json::Map::new()),
                    is_task: true,
                    task_description: None,
                    task_subagent_type: None,
                },
            ],
            tool_results: vec![],
            source_tool_use_id: None,
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            is_compact_summary: None,
            request_id: None,
        };

        let mut chunk = make_chunk(1000, 5000, vec![response]);

        let subagents = vec![
            make_subagent("sub-a", 2000, 3000, Some("task-a")),
            make_subagent("sub-b", 2100, 3100, Some("task-b")), // task-b is not is_task
            make_subagent("sub-c", 2200, 3200, Some("task-c")),
        ];

        link_processes_to_chunk(&mut chunk, &subagents);

        assert_eq!(chunk.processes.len(), 2);
        let ids: Vec<&str> = chunk.processes.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"sub-a"));
        assert!(ids.contains(&"sub-c"));
    }
}
