use crate::types::domain::MessageType;
use crate::types::jsonl::{ChatHistoryEntry, ContentBlock};
use crate::types::messages::{ParsedMessage, ToolCall, ToolResult};

// =============================================================================
// Tool Extraction
// =============================================================================

pub fn extract_tool_calls(content: &serde_json::Value) -> Vec<ToolCall> {
    let blocks = match content {
        serde_json::Value::Array(arr) => arr,
        _ => return vec![],
    };

    let mut tool_calls = vec![];
    for block in blocks {
        let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if block_type != "tool_use" {
            continue;
        }

        let id = match block.get("id").and_then(|v| v.as_str()) {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => continue,
        };
        let name = match block.get("name").and_then(|v| v.as_str()) {
            Some(name) if !name.is_empty() => name.to_string(),
            _ => continue,
        };

        let input = block.get("input").cloned().unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
        let is_task = name == "Task";

        let task_description = if is_task {
            input.get("description").and_then(|v| v.as_str()).map(String::from)
        } else {
            None
        };
        let task_subagent_type = if is_task {
            input.get("subagent_type").and_then(|v| v.as_str()).map(String::from)
        } else {
            None
        };

        tool_calls.push(ToolCall {
            id,
            name,
            input,
            is_task,
            task_description,
            task_subagent_type,
        });
    }

    tool_calls
}

pub fn extract_tool_results(content: &serde_json::Value) -> Vec<ToolResult> {
    let blocks = match content {
        serde_json::Value::Array(arr) => arr,
        _ => return vec![],
    };

    let mut results = vec![];
    for block in blocks {
        let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if block_type != "tool_result" {
            continue;
        }

        let tool_use_id = match block.get("tool_use_id").and_then(|v| v.as_str()) {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => continue,
        };

        let result_content = block.get("content").cloned().unwrap_or(serde_json::Value::String(String::new()));
        let is_error = block.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);

        results.push(ToolResult {
            tool_use_id,
            content: result_content,
            is_error,
        });
    }

    results
}

// =============================================================================
// JSONL Line Parsing
// =============================================================================

pub fn parse_jsonl_line(line: &str) -> Option<ParsedMessage> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let entry: ChatHistoryEntry = serde_json::from_str(trimmed).ok()?;
    parse_chat_history_entry(&entry)
}

fn parse_chat_history_entry(entry: &ChatHistoryEntry) -> Option<ParsedMessage> {
    // Extract uuid from entry based on variant
    let uuid = match entry {
        ChatHistoryEntry::User(u) => u.uuid.clone(),
        ChatHistoryEntry::Assistant(a) => a.uuid.clone(),
        ChatHistoryEntry::System(s) => s.uuid.clone(),
        ChatHistoryEntry::Summary(s) => s.uuid.clone(),
        ChatHistoryEntry::FileHistorySnapshot(f) => f.uuid.clone(),
        ChatHistoryEntry::QueueOperation(q) => q.uuid.clone(),
    };
    if uuid.is_empty() {
        return None;
    }

    let (message_type, content, role, usage, model, is_meta, request_id) = match entry {
        ChatHistoryEntry::User(user) => {
            let content = user.message.content.clone();
            let role = Some(user.message.role.clone());
            let is_meta = user.is_meta.unwrap_or(false);
            (
                MessageType::User,
                content,
                role,
                None,
                None,
                is_meta,
                None,
            )
        }
        ChatHistoryEntry::Assistant(assistant) => {
            let content = serde_json::to_value(&assistant.message.content).unwrap_or(serde_json::Value::Null);
            let role = Some(assistant.message.role.clone());
            let usage = Some(assistant.message.usage.clone());
            let model = Some(assistant.message.model.clone());
            let request_id = if assistant.request_id.is_empty() {
                None
            } else {
                Some(assistant.request_id.clone())
            };
            (
                MessageType::Assistant,
                content,
                role,
                usage,
                model,
                false,
                request_id,
            )
        }
        ChatHistoryEntry::System(system) => {
            (
                MessageType::System,
                serde_json::Value::Null,
                None,
                None,
                None,
                system.is_meta,
                None,
            )
        }
        ChatHistoryEntry::Summary(summary) => {
            (
                MessageType::Summary,
                serde_json::json!(summary.summary),
                None,
                None,
                None,
                true,
                None,
            )
        }
        _ => return None,
    };

    // Extract common fields from ConversationalEntry variants
    let (cwd, git_branch, is_sidechain, user_type, parent_uuid, agent_id,
         source_tool_use_id, source_tool_assistant_uuid, tool_use_result,
         is_compact_summary) = match entry {
        ChatHistoryEntry::User(user) => (
            if user.cwd.is_empty() { None } else { Some(user.cwd.clone()) },
            if user.git_branch.is_empty() { None } else { Some(user.git_branch.clone()) },
            user.is_sidechain,
            if user.user_type.is_empty() { None } else { Some(user.user_type.clone()) },
            user.parent_uuid.clone(),
            user.agent_id.clone(),
            user.source_tool_use_id.clone(),
            user.source_tool_assistant_uuid.clone(),
            user.tool_use_result.clone(),
            user.is_meta.unwrap_or(false),
        ),
        ChatHistoryEntry::Assistant(assistant) => (
            if assistant.cwd.is_empty() { None } else { Some(assistant.cwd.clone()) },
            if assistant.git_branch.is_empty() { None } else { Some(assistant.git_branch.clone()) },
            assistant.is_sidechain,
            if assistant.user_type.is_empty() { None } else { Some(assistant.user_type.clone()) },
            assistant.parent_uuid.clone(),
            assistant.agent_id.clone(),
            None,
            None,
            None,
            false,
        ),
        ChatHistoryEntry::System(system) => (
            if system.cwd.is_empty() { None } else { Some(system.cwd.clone()) },
            if system.git_branch.is_empty() { None } else { Some(system.git_branch.clone()) },
            system.is_sidechain,
            if system.user_type.is_empty() { None } else { Some(system.user_type.clone()) },
            system.parent_uuid.clone(),
            None,
            None,
            None,
            None,
            false,
        ),
        _ => (None, None, false, None, None, None, None, None, None, false),
    };

    // Extract tool calls and results from content blocks
    let tool_calls = extract_tool_calls(&content);
    let tool_results = extract_tool_results(&content);

    let timestamp = match entry {
        ChatHistoryEntry::User(u) => u.timestamp.clone().unwrap_or_default(),
        ChatHistoryEntry::Assistant(a) => a.timestamp.clone().unwrap_or_default(),
        ChatHistoryEntry::System(s) => s.timestamp.clone().unwrap_or_default(),
        ChatHistoryEntry::Summary(s) => s.timestamp.clone().unwrap_or_default(),
        _ => String::new(),
    };

    Some(ParsedMessage {
        uuid,
        parent_uuid,
        message_type,
        timestamp,
        role,
        content,
        usage,
        model,
        cwd,
        git_branch,
        agent_id,
        is_sidechain,
        is_meta,
        user_type,
        tool_calls,
        tool_results,
        source_tool_use_id,
        source_tool_assistant_uuid,
        tool_use_result,
        is_compact_summary: None,
        request_id,
    })
}

// =============================================================================
// File Parsing
// =============================================================================

pub async fn parse_jsonl_file(file_path: &std::path::Path) -> Vec<ParsedMessage> {
    if !file_path.exists() {
        return vec![];
    }

    let content = match tokio::fs::read_to_string(file_path).await {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    let mut messages = vec![];
    for line in content.lines() {
        if let Some(msg) = parse_jsonl_line(line) {
            messages.push(msg);
        }
    }

    messages
}

// =============================================================================
// Deduplication
// =============================================================================

pub fn deduplicate_by_request_id(messages: &[ParsedMessage]) -> Vec<ParsedMessage> {
    use std::collections::HashMap;

    let mut last_index_by_rid: HashMap<String, usize> = HashMap::new();
    for (i, msg) in messages.iter().enumerate() {
        if let Some(ref rid) = msg.request_id {
            if !rid.is_empty() {
                last_index_by_rid.insert(rid.clone(), i);
            }
        }
    }

    if last_index_by_rid.is_empty() {
        return messages.to_vec();
    }

    messages
        .iter()
        .enumerate()
        .filter(|(i, msg)| {
            match &msg.request_id {
                Some(rid) if !rid.is_empty() => {
                    last_index_by_rid.get(rid) == Some(i)
                }
                _ => true,
            }
        })
        .map(|(_, msg)| msg.clone())
        .collect()
}

// =============================================================================
// Metrics Calculation
// =============================================================================

pub fn calculate_metrics(messages: &[ParsedMessage]) -> crate::types::domain::SessionMetrics {
    use crate::types::domain::SessionMetrics;

    if messages.is_empty() {
        return SessionMetrics::default();
    }

    let deduped = deduplicate_by_request_id(messages);

    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut cache_read_tokens: u64 = 0;
    let mut cache_creation_tokens: u64 = 0;

    for msg in &deduped {
        if let Some(ref usage) = msg.usage {
            input_tokens += usage.input_tokens;
            output_tokens += usage.output_tokens;
            cache_read_tokens += usage.cache_read_input_tokens.unwrap_or(0);
            cache_creation_tokens += usage.cache_creation_input_tokens.unwrap_or(0);
        }
    }

    // Parse timestamps to find duration
    let mut timestamps: Vec<u64> = vec![];
    for msg in messages {
        if !msg.timestamp.is_empty() {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&msg.timestamp) {
                timestamps.push(dt.timestamp_millis() as u64);
            } else if let Ok(dt) = msg.timestamp.parse::<chrono::DateTime<chrono::Utc>>() {
                timestamps.push(dt.timestamp_millis() as u64);
            }
        }
    }

    let (min_time, max_time) = if timestamps.is_empty() {
        (0, 0)
    } else {
        let min = timestamps.iter().copied().min().unwrap_or(0);
        let max = timestamps.iter().copied().max().unwrap_or(0);
        (min, max)
    };

    SessionMetrics {
        duration_ms: max_time.saturating_sub(min_time),
        total_tokens: input_tokens + cache_creation_tokens + cache_read_tokens + output_tokens,
        input_tokens,
        output_tokens,
        cache_read_tokens: if cache_read_tokens > 0 { Some(cache_read_tokens) } else { None },
        cache_creation_tokens: if cache_creation_tokens > 0 { Some(cache_creation_tokens) } else { None },
        message_count: messages.len() as u32,
        cost_usd: None,
    }
}

// =============================================================================
// Text Extraction
// =============================================================================

pub fn extract_text_content(message: &ParsedMessage) -> String {
    match &message.content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|block| {
                let t = block.get("type")?.as_str()?;
                if t == "text" {
                    block.get("text")?.as_str().map(String::from)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

pub fn get_task_calls(messages: &[ParsedMessage]) -> Vec<ToolCall> {
    messages
        .iter()
        .flat_map(|m| m.tool_calls.iter().filter(|tc| tc.is_task).cloned())
        .collect()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_user_entry() {
        let line = r#"{
            "type":"user",
            "uuid":"u1",
            "parentUuid":null,
            "timestamp":"2026-03-25T06:00:00.000Z",
            "message":{"role":"user","content":"hello"},
            "isMeta":false,
            "isSidechain":false,
            "userType":"external",
            "cwd":"/tmp",
            "sessionId":"s1",
            "version":"1.0",
            "gitBranch":"main"
        }"#;
        let msg = parse_jsonl_line(line).unwrap();
        assert_eq!(msg.uuid, "u1");
        assert_eq!(msg.message_type, MessageType::User);
        assert!(!msg.is_meta);
    }

    #[test]
    fn test_parse_assistant_entry_with_tools() {
        let line = r#"{
            "type":"assistant",
            "uuid":"a1",
            "timestamp":"2026-03-25T06:00:01.000Z",
            "message":{
                "role":"assistant",
                "id":"msg_1",
                "type":"message",
                "model":"claude-3-opus",
                "content":[
                    {"type":"text","text":"Let me read that file."},
                    {"type":"tool_use","id":"tu_1","name":"Read","input":{"file_path":"/tmp/test.txt"}}
                ],
                "stop_reason":"end_turn",
                "usage":{"input_tokens":100,"output_tokens":50}
            },
            "requestId":"r1",
            "isSidechain":false,
            "userType":"external",
            "cwd":"/tmp",
            "sessionId":"s1",
            "version":"1.0",
            "gitBranch":"main"
        }"#;
        let msg = parse_jsonl_line(line).unwrap();
        assert_eq!(msg.message_type, MessageType::Assistant);
        assert_eq!(msg.tool_calls.len(), 1);
        assert_eq!(msg.tool_calls[0].name, "Read");
        assert_eq!(msg.usage.as_ref().unwrap().input_tokens, 100);
    }

    #[test]
    fn test_extract_tool_calls() {
        let content = serde_json::json!([
            {"type":"text","text":"hi"},
            {"type":"tool_use","id":"tu_1","name":"Bash","input":{"command":"ls"}},
            {"type":"tool_use","id":"tu_2","name":"Task","input":{"description":"do stuff","subagent_type":"Explore"}}
        ]);
        let calls = extract_tool_calls(&content);
        assert_eq!(calls.len(), 2);
        assert!(!calls[0].is_task);
        assert!(calls[1].is_task);
        assert_eq!(calls[1].task_description.as_deref(), Some("do stuff"));
    }

    #[test]
    fn test_extract_tool_results() {
        let content = serde_json::json!([
            {"type":"tool_result","tool_use_id":"tu_1","content":"output here","is_error":false}
        ]);
        let results = extract_tool_results(&content);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_use_id, "tu_1");
        assert!(!results[0].is_error);
    }

    #[test]
    fn test_deduplicate_by_request_id() {
        let msg1 = create_test_assistant_msg("a1", "r1", 100, 10);
        let msg2 = create_test_assistant_msg("a2", "r1", 100, 20); // same requestId, more tokens
        let msg3 = create_test_assistant_msg("a3", "r2", 200, 30);

        let deduped = deduplicate_by_request_id(&[msg1, msg2, msg3]);
        assert_eq!(deduped.len(), 2); // a2 (last of r1) and a3
        assert_eq!(deduped[0].uuid, "a2");
        assert_eq!(deduped[1].uuid, "a3");
    }

    #[test]
    fn test_calculate_metrics() {
        let mut msg1 = create_test_assistant_msg("a1", "r1", 100, 50);
        let mut msg2 = create_test_user_msg("u1");
        msg2.timestamp = "2026-03-25T06:00:00.000Z".to_string();
        msg1.timestamp = "2026-03-25T06:00:05.000Z".to_string();

        let metrics = calculate_metrics(&[msg2, msg1]);
        assert_eq!(metrics.input_tokens, 100);
        assert_eq!(metrics.output_tokens, 50);
        assert_eq!(metrics.message_count, 2);
        assert_eq!(metrics.duration_ms, 5000);
    }

    #[test]
    fn test_fixture_file_parsing() {
        let fixture_path = std::path::Path::new("fixtures/sample-session.jsonl");
        if !fixture_path.exists() {
            return; // Skip if no fixture
        }
        let rt = tokio::runtime::Runtime::new().unwrap();
        let messages = rt.block_on(parse_jsonl_file(fixture_path));
        assert!(!messages.is_empty(), "Should parse at least one message from fixture");
    }

    fn create_test_assistant_msg(uuid: &str, req_id: &str, input: u64, output: u64) -> ParsedMessage {
        ParsedMessage {
            uuid: uuid.to_string(),
            parent_uuid: None,
            message_type: MessageType::Assistant,
            timestamp: String::new(),
            role: Some("assistant".to_string()),
            content: serde_json::Value::String("test".to_string()),
            usage: Some(crate::types::jsonl::UsageMetadata {
                input_tokens: input,
                output_tokens: output,
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
            request_id: Some(req_id.to_string()),
        }
    }

    fn create_test_user_msg(uuid: &str) -> ParsedMessage {
        ParsedMessage {
            uuid: uuid.to_string(),
            parent_uuid: None,
            message_type: MessageType::User,
            timestamp: String::new(),
            role: Some("user".to_string()),
            content: serde_json::Value::String("hello".to_string()),
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
}
