//! WaterfallBuilder - Builds waterfall chart data from chunks and processes.
//!
//! Transforms chunk and process data into a timeline of WaterfallItems
//! suitable for visualizing session execution flow.
//!
//! Aligned with the Electron implementation at
//! `src/main/services/analysis/ChunkBuilder.ts` (buildWaterfallData method).

use std::collections::HashSet;

use crate::types::chunks::{Chunk, Process};

// =============================================================================
// Waterfall Types
// =============================================================================

/// A single item in the waterfall chart timeline.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaterfallItem {
    pub id: String,
    pub label: String,
    pub start_time: u64,
    pub end_time: u64,
    pub duration_ms: u64,
    pub level: u32,
    pub item_type: String,
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_parallel: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<WaterfallItemMetadata>,
}

/// Optional metadata for waterfall items (subagent and tool items).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaterfallItemMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_count: Option<u32>,
}

/// Complete waterfall chart data for a session.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaterfallData {
    pub items: Vec<WaterfallItem>,
    pub min_time: u64,
    pub max_time: u64,
    pub total_duration_ms: u64,
}

// =============================================================================
// Public API
// =============================================================================

/// Build waterfall chart data from chunks and resolved processes.
///
/// For each chunk, creates a level-0 WaterfallItem. For AI chunks, also creates
/// level-1 child items for tool executions and linked processes. Orphaned processes
/// (not attached to any AI chunk) are added at level 0.
pub fn build_waterfall_data(chunks: &[Chunk], processes: &[Process]) -> WaterfallData {
    let mut items: Vec<WaterfallItem> = Vec::new();
    let mut linked_process_ids: HashSet<String> = HashSet::new();

    for chunk in chunks {
        let (id, label, start_time, end_time, duration_ms) = chunk_base_info(chunk);

        let base_item = WaterfallItem {
            id: id.clone(),
            label,
            start_time,
            end_time,
            duration_ms,
            level: 0,
            item_type: "chunk".to_string(),
            parent_id: None,
            is_parallel: None,
            metadata: None,
        };
        items.push(base_item);

        if let Chunk::Ai(ai) = chunk {
            // Level-1 items for tool executions
            for tool_exec in &ai.tool_executions {
                let end_time = tool_exec
                    .end_time
                    .as_deref()
                    .and_then(parse_ts_ms)
                    .unwrap_or_else(|| parse_ts_ms(&tool_exec.start_time).unwrap_or(0));
                let start_time = parse_ts_ms(&tool_exec.start_time).unwrap_or(0);
                let duration_ms = tool_exec
                    .duration_ms
                    .unwrap_or_else(|| end_time.saturating_sub(start_time));

                items.push(WaterfallItem {
                    id: format!("tool-{}", tool_exec.tool_call.id),
                    label: tool_exec.tool_call.name.clone(),
                    start_time,
                    end_time,
                    duration_ms,
                    level: 1,
                    item_type: "tool".to_string(),
                    parent_id: Some(id.clone()),
                    is_parallel: None,
                    metadata: Some(WaterfallItemMetadata {
                        subagent_type: None,
                        tool_name: Some(tool_exec.tool_call.name.clone()),
                        message_count: None,
                    }),
                });
            }

            // Level-1 items for linked processes
            for process in &ai.processes {
                linked_process_ids.insert(process.id.clone());
                items.push(process_waterfall_item(process, Some(id.clone()), 1));
            }
        }
    }

    // Add orphaned processes (not attached to any AI chunk) at level 0
    for process in processes {
        let item_id = format!("subagent-{}", process.id);
        if linked_process_ids.contains(&process.id) {
            continue;
        }
        items.push(WaterfallItem {
            id: item_id,
            label: process_label(process),
            start_time: process.start_time,
            end_time: process.end_time,
            duration_ms: process.duration_ms,
            level: 0,
            item_type: "subagent".to_string(),
            parent_id: None,
            is_parallel: Some(process.is_parallel),
            metadata: Some(WaterfallItemMetadata {
                subagent_type: process.subagent_type.clone(),
                tool_name: None,
                message_count: Some(process.messages.len() as u32),
            }),
        });
    }

    // Sort all items by start_time
    items.sort_by_key(|item| item.start_time);

    if items.is_empty() {
        return WaterfallData {
            items: vec![],
            min_time: 0,
            max_time: 0,
            total_duration_ms: 0,
        };
    }

    let min_time = items
        .iter()
        .map(|item| item.start_time)
        .min()
        .unwrap_or(0);
    let max_time = items
        .iter()
        .map(|item| item.end_time)
        .max()
        .unwrap_or(0);
    let total_duration_ms = max_time.saturating_sub(min_time);

    WaterfallData {
        items,
        min_time,
        max_time,
        total_duration_ms,
    }
}

// =============================================================================
// Private Helpers
// =============================================================================

/// Extract base chunk info (id, label, start_time, end_time, duration_ms).
fn chunk_base_info(chunk: &Chunk) -> (String, String, u64, u64, u64) {
    match chunk {
        Chunk::User(c) => (
            c.id.clone(),
            "User".to_string(),
            c.start_time,
            c.end_time,
            c.duration_ms,
        ),
        Chunk::Ai(c) => (
            c.id.clone(),
            "Assistant".to_string(),
            c.start_time,
            c.end_time,
            c.duration_ms,
        ),
        Chunk::System(c) => (
            c.id.clone(),
            "System".to_string(),
            c.start_time,
            c.end_time,
            c.duration_ms,
        ),
        Chunk::Compact(c) => (
            c.id.clone(),
            "Compact".to_string(),
            c.start_time,
            c.end_time,
            c.duration_ms,
        ),
    }
}

/// Build a waterfall item from a process (subagent).
fn process_waterfall_item(process: &Process, parent_id: Option<String>, level: u32) -> WaterfallItem {
    WaterfallItem {
        id: format!("subagent-{}", process.id),
        label: process_label(process),
        start_time: process.start_time,
        end_time: process.end_time,
        duration_ms: process.duration_ms,
        level,
        item_type: "subagent".to_string(),
        parent_id,
        is_parallel: Some(process.is_parallel),
        metadata: Some(WaterfallItemMetadata {
            subagent_type: process.subagent_type.clone(),
            tool_name: None,
            message_count: Some(process.messages.len() as u32),
        }),
    }
}

/// Determine the display label for a process.
fn process_label(process: &Process) -> String {
    process
        .description
        .clone()
        .or_else(|| process.subagent_type.clone())
        .unwrap_or_else(|| process.id.clone())
}

/// Parse an RFC 3339 timestamp string to epoch milliseconds.
fn parse_ts_ms(ts: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.timestamp_millis() as u64)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::chunks::{
        AiChunk, Chunk, CompactChunk, Process, SystemChunk, ToolExecution, UserChunk,
    };
    use crate::types::domain::SessionMetrics;
    use crate::types::messages::{ParsedMessage, ToolCall};

    fn make_user_chunk(id: &str, start: u64, end: u64) -> Chunk {
        Chunk::User(UserChunk {
            id: id.to_string(),
            start_time: start,
            end_time: end,
            duration_ms: end.saturating_sub(start),
            metrics: SessionMetrics::default(),
            user_message: make_parsed_message("user-msg", "2026-01-01T00:00:00Z"),
        })
    }

    fn make_ai_chunk(
        id: &str,
        start: u64,
        end: u64,
        tool_executions: Vec<ToolExecution>,
        processes: Vec<Process>,
    ) -> Chunk {
        Chunk::Ai(AiChunk {
            id: id.to_string(),
            start_time: start,
            end_time: end,
            duration_ms: end.saturating_sub(start),
            metrics: SessionMetrics::default(),
            responses: vec![],
            processes,
            sidechain_messages: vec![],
            tool_executions,
            semantic_steps: vec![],
            semantic_step_groups: vec![],
        })
    }

    fn make_system_chunk(id: &str, start: u64, end: u64) -> Chunk {
        Chunk::System(SystemChunk {
            id: id.to_string(),
            start_time: start,
            end_time: end,
            duration_ms: end.saturating_sub(start),
            metrics: SessionMetrics::default(),
            message: make_parsed_message("sys-msg", "2026-01-01T00:00:00Z"),
            command_output: String::new(),
        })
    }

    fn make_compact_chunk(id: &str, start: u64, end: u64) -> Chunk {
        Chunk::Compact(CompactChunk {
            id: id.to_string(),
            start_time: start,
            end_time: end,
            duration_ms: end.saturating_sub(start),
            metrics: SessionMetrics::default(),
            message: make_parsed_message("compact-msg", "2026-01-01T00:00:00Z"),
        })
    }

    fn make_tool_execution(id: &str, name: &str, start: &str, end: Option<&str>) -> ToolExecution {
        let duration_ms = end.and_then(|e| parse_ts_ms(start).and_then(|s| {
            parse_ts_ms(e).map(|end_ms| end_ms.saturating_sub(s))
        }));
        ToolExecution {
            tool_call: ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                input: serde_json::Value::Object(serde_json::Map::new()),
                is_task: false,
                task_description: None,
                task_subagent_type: None,
            },
            result: None,
            start_time: start.to_string(),
            end_time: end.map(String::from),
            duration_ms,
        }
    }

    fn make_process(
        id: &str,
        start: u64,
        end: u64,
        description: Option<&str>,
        subagent_type: Option<&str>,
    ) -> Process {
        Process {
            id: id.to_string(),
            file_path: format!("/path/to/{}", id),
            description: description.map(String::from),
            subagent_type: subagent_type.map(String::from),
            messages: vec![],
            start_time: start,
            end_time: end,
            duration_ms: end.saturating_sub(start),
            metrics: SessionMetrics::default(),
            is_parallel: false,
            parent_task_id: None,
            is_ongoing: None,
            main_session_impact: None,
            team: None,
        }
    }

    fn make_parsed_message(uuid: &str, ts: &str) -> ParsedMessage {
        ParsedMessage {
            uuid: uuid.to_string(),
            parent_uuid: None,
            message_type: crate::types::domain::MessageType::User,
            timestamp: ts.to_string(),
            role: Some("user".to_string()),
            content: serde_json::Value::String("msg".to_string()),
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

    // =========================================================================
    // Basic tests
    // =========================================================================

    #[test]
    fn test_empty_chunks_and_processes() {
        let data = build_waterfall_data(&[], &[]);
        assert!(data.items.is_empty());
        assert_eq!(data.min_time, 0);
        assert_eq!(data.max_time, 0);
        assert_eq!(data.total_duration_ms, 0);
    }

    #[test]
    fn test_single_user_chunk() {
        let chunks = vec![make_user_chunk("u1", 1000, 2000)];
        let data = build_waterfall_data(&chunks, &[]);

        assert_eq!(data.items.len(), 1);
        assert_eq!(data.items[0].id, "u1");
        assert_eq!(data.items[0].label, "User");
        assert_eq!(data.items[0].start_time, 1000);
        assert_eq!(data.items[0].end_time, 2000);
        assert_eq!(data.items[0].duration_ms, 1000);
        assert_eq!(data.items[0].level, 0);
        assert_eq!(data.items[0].item_type, "chunk");
        assert!(data.items[0].parent_id.is_none());
    }

    #[test]
    fn test_chunk_labels() {
        let chunks = vec![
            make_user_chunk("u1", 1000, 2000),
            make_ai_chunk("a1", 2000, 3000, vec![], vec![]),
            make_system_chunk("s1", 3000, 4000),
            make_compact_chunk("c1", 4000, 5000),
        ];
        let data = build_waterfall_data(&chunks, &[]);

        assert_eq!(data.items.len(), 4);
        assert_eq!(data.items[0].label, "User");
        assert_eq!(data.items[1].label, "Assistant");
        assert_eq!(data.items[2].label, "System");
        assert_eq!(data.items[3].label, "Compact");
    }

    #[test]
    fn test_sorted_by_start_time() {
        let chunks = vec![
            make_user_chunk("u2", 3000, 4000),
            make_user_chunk("u1", 1000, 2000),
            make_user_chunk("u3", 5000, 6000),
        ];
        let data = build_waterfall_data(&chunks, &[]);

        assert_eq!(data.items.len(), 3);
        assert_eq!(data.items[0].id, "u1");
        assert_eq!(data.items[1].id, "u2");
        assert_eq!(data.items[2].id, "u3");
    }

    #[test]
    fn test_min_max_time_and_total_duration() {
        let chunks = vec![
            make_user_chunk("u1", 1000, 2000),
            make_user_chunk("u2", 3000, 5000),
        ];
        let data = build_waterfall_data(&chunks, &[]);

        assert_eq!(data.min_time, 1000);
        assert_eq!(data.max_time, 5000);
        assert_eq!(data.total_duration_ms, 4000);
    }

    // =========================================================================
    // AI chunk child items
    // =========================================================================

    #[test]
    fn test_ai_chunk_with_tool_executions() {
        let tool_exec = make_tool_execution(
            "tc1",
            "Read",
            "2026-01-01T00:00:01Z",
            Some("2026-01-01T00:00:03Z"),
        );
        let chunks = vec![make_ai_chunk("a1", 1000, 4000, vec![tool_exec], vec![])];
        let data = build_waterfall_data(&chunks, &[]);

        assert_eq!(data.items.len(), 2);

        // First item is the AI chunk at level 0
        assert_eq!(data.items[0].id, "a1");
        assert_eq!(data.items[0].level, 0);
        assert_eq!(data.items[0].item_type, "chunk");

        // Second item is the tool at level 1
        assert_eq!(data.items[1].id, "tool-tc1");
        assert_eq!(data.items[1].label, "Read");
        assert_eq!(data.items[1].level, 1);
        assert_eq!(data.items[1].item_type, "tool");
        assert_eq!(data.items[1].parent_id.as_deref(), Some("a1"));
        assert_eq!(data.items[1].duration_ms, 2000);
    }

    #[test]
    fn test_ai_chunk_with_linked_processes() {
        let process = make_process("sub-1", 1500, 3000, Some("Helper agent"), Some("Code"));
        let chunks = vec![make_ai_chunk("a1", 1000, 4000, vec![], vec![process])];
        let data = build_waterfall_data(&chunks, &[]);

        assert_eq!(data.items.len(), 2);

        assert_eq!(data.items[0].id, "a1");
        assert_eq!(data.items[0].level, 0);

        assert_eq!(data.items[1].id, "subagent-sub-1");
        assert_eq!(data.items[1].label, "Helper agent");
        assert_eq!(data.items[1].level, 1);
        assert_eq!(data.items[1].item_type, "subagent");
        assert_eq!(data.items[1].parent_id.as_deref(), Some("a1"));
        assert_eq!(data.items[1].is_parallel, Some(false));
    }

    #[test]
    fn test_ai_chunk_with_tool_and_process() {
        let tool_exec = make_tool_execution(
            "tc1",
            "Bash",
            "2026-01-01T00:00:01Z",
            Some("2026-01-01T00:00:02Z"),
        );
        // Use timestamps in the same range as the RFC3339-parsed tool times
        let base_ts = parse_ts_ms("2026-01-01T00:00:00Z").unwrap();
        let process = make_process("sub-1", base_ts + 1500, base_ts + 3000, None, None);
        let chunks = vec![make_ai_chunk("a1", base_ts + 1000, base_ts + 4000, vec![tool_exec], vec![process])];
        let data = build_waterfall_data(&chunks, &[]);

        // AI chunk + tool + process = 3 items
        assert_eq!(data.items.len(), 3);
        assert_eq!(data.items[0].item_type, "chunk");
        assert_eq!(data.items[1].item_type, "tool");
        assert_eq!(data.items[2].item_type, "subagent");
    }

    // =========================================================================
    // Orphaned processes
    // =========================================================================

    #[test]
    fn test_orphaned_processes_at_level_0() {
        let orphan = make_process("orphan-1", 5000, 7000, Some("Orphan"), None);
        let chunks = vec![make_user_chunk("u1", 1000, 2000)];
        let data = build_waterfall_data(&chunks, &[orphan]);

        assert_eq!(data.items.len(), 2);
        assert_eq!(data.items[1].id, "subagent-orphan-1");
        assert_eq!(data.items[1].label, "Orphan");
        assert_eq!(data.items[1].level, 0);
        assert_eq!(data.items[1].item_type, "subagent");
        assert!(data.items[1].parent_id.is_none());
    }

    #[test]
    fn test_linked_process_not_duplicated_as_orphan() {
        let process = make_process("sub-1", 1500, 3000, None, None);
        let chunks = vec![make_ai_chunk("a1", 1000, 4000, vec![], vec![process.clone()])];
        let data = build_waterfall_data(&chunks, &[process]);

        // The process should appear only once (as a child of the AI chunk)
        assert_eq!(data.items.len(), 2);
        let subagent_items: Vec<_> = data
            .items
            .iter()
            .filter(|i| i.item_type == "subagent")
            .collect();
        assert_eq!(subagent_items.len(), 1);
        assert_eq!(subagent_items[0].level, 1);
    }

    // =========================================================================
    // Process label fallbacks
    // =========================================================================

    #[test]
    fn test_process_label_description_fallback() {
        let with_desc = make_process("p1", 1000, 2000, Some("My agent"), Some("Code"));
        assert_eq!(process_label(&with_desc), "My agent");

        let no_desc = make_process("p2", 1000, 2000, None, Some("Research"));
        assert_eq!(process_label(&no_desc), "Research");

        let fallback = make_process("p3", 1000, 2000, None, None);
        assert_eq!(process_label(&fallback), "p3");
    }

    // =========================================================================
    // Tool execution edge cases
    // =========================================================================

    #[test]
    fn test_tool_execution_no_end_time() {
        let tool_exec = make_tool_execution("tc1", "Read", "2026-01-01T00:00:01Z", None);
        let chunks = vec![make_ai_chunk("a1", 1000, 4000, vec![tool_exec], vec![])];
        let data = build_waterfall_data(&chunks, &[]);

        assert_eq!(data.items.len(), 2);
        let tool_item = &data.items[1];
        assert_eq!(tool_item.id, "tool-tc1");
        // When no end_time, start == end, duration == 0
        assert_eq!(tool_item.duration_ms, 0);
    }

    #[test]
    fn test_tool_execution_with_metadata() {
        let tool_exec = make_tool_execution(
            "tc1",
            "Grep",
            "2026-01-01T00:00:01Z",
            Some("2026-01-01T00:00:02Z"),
        );
        let chunks = vec![make_ai_chunk("a1", 1000, 4000, vec![tool_exec], vec![])];
        let data = build_waterfall_data(&chunks, &[]);

        let tool_item = &data.items[1];
        assert!(tool_item.metadata.is_some());
        let meta = tool_item.metadata.as_ref().unwrap();
        assert_eq!(meta.tool_name.as_deref(), Some("Grep"));
        assert!(meta.subagent_type.is_none());
    }

    // =========================================================================
    // Mixed scenarios
    // =========================================================================

    #[test]
    fn test_mixed_chunks_with_orphaned_process() {
        let chunks = vec![
            make_user_chunk("u1", 1000, 2000),
            make_ai_chunk("a1", 2000, 5000, vec![], vec![]),
            make_system_chunk("s1", 5000, 6000),
        ];
        let orphan = make_process("orphan-1", 6500, 8000, None, None);
        let data = build_waterfall_data(&chunks, &[orphan]);

        assert_eq!(data.items.len(), 4);
        // Items should be sorted by start_time
        assert_eq!(data.items[0].id, "u1");
        assert_eq!(data.items[1].id, "a1");
        assert_eq!(data.items[2].id, "s1");
        assert_eq!(data.items[3].id, "subagent-orphan-1");

        assert_eq!(data.min_time, 1000);
        assert_eq!(data.max_time, 8000);
        assert_eq!(data.total_duration_ms, 7000);
    }

    #[test]
    fn test_process_metadata() {
        let process = Process {
            id: "sub-1".to_string(),
            file_path: "/path/to/sub-1".to_string(),
            description: Some("Helper".to_string()),
            subagent_type: Some("Code".to_string()),
            messages: vec![
                make_parsed_message("m1", "2026-01-01T00:00:00Z"),
                make_parsed_message("m2", "2026-01-01T00:00:01Z"),
            ],
            start_time: 1500,
            end_time: 3000,
            duration_ms: 1500,
            metrics: SessionMetrics::default(),
            is_parallel: true,
            parent_task_id: None,
            is_ongoing: None,
            main_session_impact: None,
            team: None,
        };
        let chunks = vec![make_ai_chunk("a1", 1000, 4000, vec![], vec![process])];
        let data = build_waterfall_data(&chunks, &[]);

        let subagent = &data.items[1];
        assert_eq!(subagent.is_parallel, Some(true));
        let meta = subagent.metadata.as_ref().unwrap();
        assert_eq!(meta.subagent_type.as_deref(), Some("Code"));
        assert_eq!(meta.message_count, Some(2));
    }

    // =========================================================================
    // parse_ts_ms tests
    // =========================================================================

    #[test]
    fn test_parse_ts_ms_valid() {
        assert_eq!(
            parse_ts_ms("2026-01-01T00:00:00Z"),
            Some(1767225600000)
        );
    }

    #[test]
    fn test_parse_ts_ms_invalid() {
        assert_eq!(parse_ts_ms("not-a-timestamp"), None);
    }

    #[test]
    fn test_parse_ts_ms_empty() {
        assert_eq!(parse_ts_ms(""), None);
    }
}
