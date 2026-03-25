//! Tests for ChunkBuilder

use crate::analysis::ChunkBuilder;
use crate::types::chunks::{AiChunk, Chunk, UserChunk};
use crate::types::domain::{MessageType, SessionMetrics};
use crate::types::messages::ParsedMessage;

fn make_user_msg(uuid: &str, ts: &str) -> ParsedMessage {
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

fn make_hard_noise_msg(uuid: &str, ts: &str) -> ParsedMessage {
    ParsedMessage {
        uuid: uuid.to_string(),
        parent_uuid: None,
        message_type: MessageType::System,
        timestamp: ts.to_string(),
        role: None,
        content: serde_json::Value::String("<system-reminder>noise</system-reminder>".to_string()),
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

#[test]
fn test_build_chunks_mixed_messages() {
    let messages = vec![
        make_user_msg("u1", "2026-01-01T00:00:00Z"),
        make_assistant_msg("a1", "2026-01-01T00:00:01Z"),
        make_hard_noise_msg("n1", "2026-01-01T00:00:02Z"),
        make_user_msg("u2", "2026-01-01T00:00:03Z"),
    ];

    let chunks = ChunkBuilder::build_chunks(&messages, &[]);

    assert_eq!(chunks.len(), 3);
    assert!(matches!(&chunks[0], Chunk::User(_)));
    assert!(matches!(&chunks[1], Chunk::Ai(_)));
    assert!(matches!(&chunks[2], Chunk::User(_)));
}

#[test]
fn test_ai_buffer_flush() {
    let messages = vec![
        make_assistant_msg("a1", "2026-01-01T00:00:00Z"),
        make_assistant_msg("a2", "2026-01-01T00:00:01Z"),
        make_user_msg("u1", "2026-01-01T00:00:02Z"),
    ];

    let chunks = ChunkBuilder::build_chunks(&messages, &[]);

    assert_eq!(chunks.len(), 2);
    if let Chunk::Ai(ai_chunk) = &chunks[0] {
        assert_eq!(ai_chunk.responses.len(), 2);
    } else {
        panic!("Expected AI chunk");
    }
}

#[test]
fn test_hard_noise_filtering() {
    let messages = vec![
        make_hard_noise_msg("n1", "2026-01-01T00:00:00Z"),
        make_hard_noise_msg("n2", "2026-01-01T00:00:01Z"),
        make_user_msg("u1", "2026-01-01T00:00:02Z"),
    ];

    let chunks = ChunkBuilder::build_chunks(&messages, &[]);

    assert_eq!(chunks.len(), 1);
    assert!(matches!(&chunks[0], Chunk::User(_)));
}

#[test]
fn test_get_total_chunk_metrics() {
    let chunks = vec![
        Chunk::User(UserChunk {
            id: "c1".to_string(),
            start_time: String::new(),
            end_time: String::new(),
            duration_ms: 100,
            metrics: SessionMetrics {
                duration_ms: 100,
                total_tokens: 50,
                input_tokens: 30,
                output_tokens: 20,
                cache_read_tokens: None,
                cache_creation_tokens: None,
                message_count: 1,
                cost_usd: None,
            },
            user_message: make_user_msg("u1", "2026-01-01T00:00:00Z"),
        }),
        Chunk::Ai(AiChunk {
            id: "c2".to_string(),
            start_time: String::new(),
            end_time: String::new(),
            duration_ms: 200,
            metrics: SessionMetrics {
                duration_ms: 200,
                total_tokens: 100,
                input_tokens: 60,
                output_tokens: 40,
                cache_read_tokens: Some(10),
                cache_creation_tokens: None,
                message_count: 2,
                cost_usd: None,
            },
            responses: vec![],
            processes: vec![],
            sidechain_messages: vec![],
            tool_executions: vec![],
        }),
    ];

    let metrics = ChunkBuilder::get_total_chunk_metrics(&chunks);

    assert_eq!(metrics.duration_ms, 300);
    assert_eq!(metrics.input_tokens, 90);
    assert_eq!(metrics.output_tokens, 60);
    assert_eq!(metrics.cache_read_tokens, Some(10));
    assert_eq!(metrics.message_count, 3);
}