use crate::analysis::process_linker::link_processes_to_chunk;
use crate::analysis::semantic_step_extractor::extract_semantic_steps;
use crate::analysis::semantic_step_grouper::build_semantic_step_groups;
use crate::analysis::tool_execution_builder::build_tool_executions;
use crate::parsing::{calculate_metrics, classify_message};
use crate::types::chunks::{
    AiChunk, Chunk, CompactChunk, Process, SessionDetail, SystemChunk, UserChunk,
};
use crate::types::domain::{MessageCategory, Session, SessionMetrics};
use crate::types::messages::ParsedMessage;
use crate::utils::context_accumulator::calculate_step_context;
use crate::utils::timeline_gap_filling::{fill_timeline_gaps, GapFillingInput};

/// ChunkBuilder service - Builds visualization chunks from parsed session data.
pub struct ChunkBuilder;

impl ChunkBuilder {
    /// Build chunks from messages using 4-category classification.
    ///
    /// Produces stable `{type}-{uuid}` IDs instead of positional `chunk-{N}` IDs.
    /// Collects sidechain messages within each AI chunk's time range.
    /// Runs the full semantic step pipeline (extract, gap-fill, context, group).
    pub fn build_chunks(messages: &[ParsedMessage], subagents: &[Process]) -> Vec<Chunk> {
        let mut chunks: Vec<Chunk> = Vec::new();
        let main_messages: Vec<&ParsedMessage> = messages.iter().filter(|m| !m.is_sidechain).collect();
        let mut ai_buffer: Vec<ParsedMessage> = Vec::new();

        for message in main_messages {
            match classify_message(message) {
                MessageCategory::HardNoise => {}
                MessageCategory::Compact => {
                    Self::flush_ai(&mut ai_buffer, &mut chunks, subagents, messages);
                    chunks.push(Self::compact_chunk(message));
                }
                MessageCategory::User => {
                    Self::flush_ai(&mut ai_buffer, &mut chunks, subagents, messages);
                    chunks.push(Self::user_chunk(message));
                }
                MessageCategory::System => {
                    Self::flush_ai(&mut ai_buffer, &mut chunks, subagents, messages);
                    chunks.push(Self::system_chunk(message));
                }
                MessageCategory::Ai => ai_buffer.push(message.clone()),
            }
        }

        if !ai_buffer.is_empty() {
            Self::flush_ai(&mut ai_buffer, &mut chunks, subagents, messages);
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

    fn flush_ai(
        buf: &mut Vec<ParsedMessage>,
        chunks: &mut Vec<Chunk>,
        subs: &[Process],
        all_messages: &[ParsedMessage],
    ) {
        if !buf.is_empty() {
            Self::build_ai_chunk(buf, subs, all_messages, chunks);
            buf.clear();
        }
    }

    fn build_ai_chunk(
        buf: &[ParsedMessage],
        subs: &[Process],
        all_messages: &[ParsedMessage],
        chunks: &mut Vec<Chunk>,
    ) {
        if buf.is_empty() { return; }

        let id = format!("ai-{}", buf.first().map(|m| m.uuid.as_str()).unwrap_or("empty"));
        let start = buf.first().and_then(|m| parse_ts_ms(&m.timestamp)).unwrap_or(0);
        let end = buf.last().and_then(|m| parse_ts_ms(&m.timestamp)).unwrap_or(0);

        let mut ai = AiChunk {
            id,
            start_time: start,
            end_time: end,
            duration_ms: Self::duration_ms(buf),
            metrics: calculate_metrics(buf),
            responses: buf.to_vec(),
            processes: vec![],
            sidechain_messages: Self::collect_sidechains(all_messages, start, end),
            tool_executions: build_tool_executions(buf),
            semantic_steps: vec![],
            semantic_step_groups: vec![],
        };

        // Two-tier process linking
        link_processes_to_chunk(&mut ai, subs);

        // Semantic step pipeline
        let mut steps = extract_semantic_steps(&ai);
        fill_timeline_gaps(GapFillingInput {
            steps: &mut steps,
            chunk_start_time_ms: start,
            chunk_end_time_ms: end,
        });
        calculate_step_context(&mut steps, buf);
        ai.semantic_steps = steps;
        ai.semantic_step_groups = build_semantic_step_groups(&ai.semantic_steps);

        chunks.push(Chunk::Ai(ai));
    }

    fn user_chunk(msg: &ParsedMessage) -> Chunk {
        let ts = parse_ts_ms(&msg.timestamp).unwrap_or(0);
        Chunk::User(UserChunk {
            id: format!("user-{}", msg.uuid),
            start_time: ts,
            end_time: ts,
            duration_ms: 0,
            metrics: Self::single_msg_metrics(),
            user_message: msg.clone(),
        })
    }

    fn system_chunk(msg: &ParsedMessage) -> Chunk {
        let ts = parse_ts_ms(&msg.timestamp).unwrap_or(0);
        Chunk::System(SystemChunk {
            id: format!("system-{}", msg.uuid),
            start_time: ts,
            end_time: ts,
            duration_ms: 0,
            metrics: Self::single_msg_metrics(),
            message: msg.clone(),
            command_output: Self::extract_command_output(msg),
        })
    }

    fn compact_chunk(msg: &ParsedMessage) -> Chunk {
        let ts = parse_ts_ms(&msg.timestamp).unwrap_or(0);
        Chunk::Compact(CompactChunk {
            id: format!("compact-{}", msg.uuid),
            start_time: ts,
            end_time: ts,
            duration_ms: 0,
            metrics: Self::single_msg_metrics(),
            message: msg.clone(),
        })
    }

    /// Collect sidechain messages whose timestamps fall within the chunk's time range.
    fn collect_sidechains(
        messages: &[ParsedMessage],
        chunk_start: u64,
        chunk_end: u64,
    ) -> Vec<ParsedMessage> {
        messages
            .iter()
            .filter(|m| {
                if !m.is_sidechain { return false; }
                let ts = parse_ts_ms(&m.timestamp).unwrap_or(0);
                ts >= chunk_start && ts <= chunk_end
            })
            .cloned()
            .collect()
    }

    /// Extract command output from system messages using regex.
    ///
    /// Looks for `<local-command-stdout>` and `<local-command-stderr>` tags.
    /// Falls back to the raw content string if no tags are found.
    fn extract_command_output(msg: &ParsedMessage) -> String {
        let content = match &msg.content {
            serde_json::Value::String(s) => s.as_str(),
            _ => return String::new(),
        };

        let stdout_re = regex::Regex::new(
            r"<local-command-stdout>([\s\S]*?)</local-command-stdout>",
        )
        .unwrap();
        if let Some(caps) = stdout_re.captures(content) {
            return caps
                .get(1)
                .map(|m| m.as_str())
                .unwrap_or("")
                .to_string();
        }

        let stderr_re = regex::Regex::new(
            r"<local-command-stderr>([\s\S]*?)</local-command-stderr>",
        )
        .unwrap();
        if let Some(caps) = stderr_re.captures(content) {
            return caps
                .get(1)
                .map(|m| m.as_str())
                .unwrap_or("")
                .to_string();
        }

        content.to_string()
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
}

fn parse_ts_ms(ts: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(ts).ok().map(|dt| dt.timestamp_millis() as u64)
}
