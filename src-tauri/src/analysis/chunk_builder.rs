use crate::parsing::{calculate_metrics, classify_message};
use crate::types::chunks::{
    AiChunk, Chunk, CompactChunk, Process, SessionDetail, SystemChunk, ToolExecution, UserChunk,
};
use crate::types::domain::{MessageCategory, Session, SessionMetrics};
use crate::types::messages::ParsedMessage;

/// ChunkBuilder service - Builds visualization chunks from parsed session data.
pub struct ChunkBuilder;

impl ChunkBuilder {
    /// Build chunks from messages using 4-category classification.
    pub fn build_chunks(messages: &[ParsedMessage], subagents: &[Process]) -> Vec<Chunk> {
        let mut chunks: Vec<Chunk> = Vec::new();
        let main_messages: Vec<&ParsedMessage> = messages.iter().filter(|m| !m.is_sidechain).collect();
        let mut ai_buffer: Vec<ParsedMessage> = Vec::new();
        let mut counter = 0u32;

        for message in main_messages {
            match classify_message(message) {
                MessageCategory::HardNoise => {}
                MessageCategory::Compact => {
                    Self::flush_ai(&mut ai_buffer, &mut chunks, subagents, &mut counter);
                    chunks.push(Self::compact_chunk(message, &mut counter));
                }
                MessageCategory::User => {
                    Self::flush_ai(&mut ai_buffer, &mut chunks, subagents, &mut counter);
                    chunks.push(Self::user_chunk(message, &mut counter));
                }
                MessageCategory::System => {
                    Self::flush_ai(&mut ai_buffer, &mut chunks, subagents, &mut counter);
                    chunks.push(Self::system_chunk(message, &mut counter));
                }
                MessageCategory::Ai => ai_buffer.push(message.clone()),
            }
        }

        if !ai_buffer.is_empty() {
            Self::ai_chunk(&ai_buffer, subagents, &mut chunks, &mut counter);
        }
        chunks
    }

    /// Build a complete SessionDetail from parsed data.
    pub fn build_session_detail(
        session: Session,
        messages: Vec<ParsedMessage>,
        subagents: Vec<Process>,
    ) -> SessionDetail {
        let chunks = Self::build_chunks(&messages, &subagents);
        let metrics = calculate_metrics(&messages);
        SessionDetail { session, messages, chunks, processes: subagents, metrics }
    }

    /// Get total metrics for all chunks.
    pub fn get_total_chunk_metrics(chunks: &[Chunk]) -> SessionMetrics {
        if chunks.is_empty() { return SessionMetrics::default(); }

        let (mut dur, mut in_tok, mut out_tok) = (0u64, 0u64, 0u64);
        let (mut cache_read, mut cache_create, mut msg_count) = (None, None, 0u32);

        for chunk in chunks {
            let m = Self::chunk_metrics(chunk);
            dur += m.duration_ms; in_tok += m.input_tokens; out_tok += m.output_tokens;
            cache_read = Self::merge_opt(cache_read, m.cache_read_tokens);
            cache_create = Self::merge_opt(cache_create, m.cache_creation_tokens);
            msg_count += m.message_count;
        }

        SessionMetrics {
            duration_ms: dur, total_tokens: in_tok + out_tok, input_tokens: in_tok,
            output_tokens: out_tok, cache_read_tokens: cache_read, cache_creation_tokens: cache_create,
            message_count: msg_count, cost_usd: None,
        }
    }

    fn flush_ai(buf: &mut Vec<ParsedMessage>, chunks: &mut Vec<Chunk>, subs: &[Process], ctr: &mut u32) {
        if !buf.is_empty() { Self::ai_chunk(buf, subs, chunks, ctr); buf.clear(); }
    }

    fn ai_chunk(buf: &[ParsedMessage], subs: &[Process], chunks: &mut Vec<Chunk>, ctr: &mut u32) {
        if buf.is_empty() { return; }
        *ctr += 1;
        let start = buf.first().and_then(|m| parse_ts_ms(&m.timestamp)).unwrap_or(0);
        let end = buf.last().and_then(|m| parse_ts_ms(&m.timestamp)).unwrap_or(0);
        chunks.push(Chunk::Ai(AiChunk {
            id: format!("chunk-{}", ctr), start_time: start, end_time: end,
            duration_ms: Self::duration_ms(buf), metrics: calculate_metrics(buf),
            responses: buf.to_vec(), processes: Self::link_subs(buf, subs),
            sidechain_messages: vec![], tool_executions: Self::tool_execs(buf),
            semantic_steps: vec![], semantic_step_groups: vec![],
        }));
    }

    fn user_chunk(msg: &ParsedMessage, ctr: &mut u32) -> Chunk {
        *ctr += 1;
        let ts = parse_ts_ms(&msg.timestamp).unwrap_or(0);
        Chunk::User(UserChunk {
            id: format!("chunk-{}", ctr), start_time: ts, end_time: ts, duration_ms: 0,
            metrics: Self::single_msg_metrics(), user_message: msg.clone(),
        })
    }

    fn system_chunk(msg: &ParsedMessage, ctr: &mut u32) -> Chunk {
        *ctr += 1;
        let ts = parse_ts_ms(&msg.timestamp).unwrap_or(0);
        Chunk::System(SystemChunk {
            id: format!("chunk-{}", ctr), start_time: ts, end_time: ts, duration_ms: 0,
            metrics: Self::single_msg_metrics(), message: msg.clone(),
            command_output: Self::cmd_output(msg),
        })
    }

    fn compact_chunk(msg: &ParsedMessage, ctr: &mut u32) -> Chunk {
        *ctr += 1;
        let ts = parse_ts_ms(&msg.timestamp).unwrap_or(0);
        Chunk::Compact(CompactChunk {
            id: format!("chunk-{}", ctr), start_time: ts, end_time: ts, duration_ms: 0,
            metrics: Self::single_msg_metrics(), message: msg.clone(),
        })
    }

    fn chunk_metrics(chunk: &Chunk) -> &SessionMetrics {
        match chunk { Chunk::User(c) => &c.metrics, Chunk::Ai(c) => &c.metrics,
            Chunk::System(c) => &c.metrics, Chunk::Compact(c) => &c.metrics }
    }

    fn single_msg_metrics() -> SessionMetrics {
        SessionMetrics { duration_ms: 0, total_tokens: 0, input_tokens: 0, output_tokens: 0,
            cache_read_tokens: None, cache_creation_tokens: None, message_count: 1, cost_usd: None }
    }

    fn merge_opt(a: Option<u64>, b: Option<u64>) -> Option<u64> {
        match (a, b) { (None, None) => None, (Some(x), None) | (None, Some(x)) => Some(x),
            (Some(x), Some(y)) => Some(x + y) }
    }

    fn duration_ms(msgs: &[ParsedMessage]) -> u64 {
        if msgs.len() < 2 { return 0; }
        let first = msgs.first().and_then(|m| parse_ts_ms(&m.timestamp));
        let last = msgs.last().and_then(|m| parse_ts_ms(&m.timestamp));
        match (first, last) { (Some(f), Some(l)) => l.saturating_sub(f), _ => 0 }
    }

    fn cmd_output(msg: &ParsedMessage) -> String {
        match &msg.content { serde_json::Value::String(s) => s.clone(), _ => String::new() }
    }

    fn link_subs(buf: &[ParsedMessage], subs: &[Process]) -> Vec<Process> {
        let task_ids: Vec<_> = buf.iter().flat_map(|m| m.tool_calls.iter())
            .filter(|tc| tc.is_task).map(|tc| tc.id.clone()).collect();
        subs.iter().filter(|p| p.parent_task_id.as_ref()
            .map_or(false, |tid| task_ids.contains(tid))).cloned().collect()
    }

    fn tool_execs(buf: &[ParsedMessage]) -> Vec<ToolExecution> {
        let mut execs = Vec::new();
        for msg in buf {
            for tc in &msg.tool_calls {
                let result = buf.iter().flat_map(|m| m.tool_results.iter())
                    .find(|tr| tr.tool_use_id == tc.id).cloned();
                execs.push(ToolExecution { tool_call: tc.clone(), result,
                    start_time: msg.timestamp.clone(), end_time: None, duration_ms: None });
            }
        }
        execs
    }
}

fn parse_ts_ms(ts: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(ts).ok().map(|dt| dt.timestamp_millis() as u64)
}
