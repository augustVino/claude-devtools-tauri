//! Subagent identity functions - file listing, parsing, timing, metrics, and parallel detection.
//!
//! These are free functions that operate on subagent data. They were extracted from
//! `subagent_resolver.rs` to keep the main file focused on orchestration.

use crate::parsing::jsonl_parser::parse_jsonl_content;
use crate::types::domain::{MessageType, SessionMetrics};
use crate::types::messages::ParsedMessage;
use std::path::{Path, PathBuf};

/// Parallel detection window in milliseconds
pub(super) const PARALLEL_WINDOW_MS: u64 = 100;

/// Check if a JSONL file belongs to a specific session (for OLD directory structure).
pub(super) fn subagent_belongs_to_session(file_content: &str, target_session_id: &str) -> bool {
    let first_line = file_content.lines().next().unwrap_or("");
    let json: serde_json::Value = match serde_json::from_str(first_line) {
        Ok(v) => v,
        Err(_) => return false,
    };
    json.get("sessionId")
        .and_then(|v| v.as_str())
        .map(|s| s == target_session_id)
        .unwrap_or(false)
}

/// List subagent files for a session.
///
/// Scans two directory structures:
/// - **Phase 1 (NEW)**: `{projectId}/{sessionId}/subagents/agent-{id}.jsonl`
/// - **Phase 2 (OLD)**: `{projectId}/agent-{id}.jsonl` (matched by sessionId in first line)
pub(super) fn list_subagent_files(
    projects_dir: &PathBuf,
    fs_provider: &dyn crate::infrastructure::fs_provider::FsProvider,
    project_id: &str,
    session_id: &str,
) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let base_dir = crate::utils::path_decoder::extract_base_dir(project_id);

    // Phase 1: NEW structure scan
    let subagents_dir = projects_dir
        .join(&base_dir)
        .join(session_id)
        .join("subagents");

    if let Ok(entries) = fs_provider.read_dir(&subagents_dir) {
        files.extend(entries.into_iter().filter_map(|dirent| {
            if dirent.is_file
                && dirent.name.ends_with(".jsonl")
                && dirent.name.starts_with("agent-")
                && !dirent.name.contains("acompact")
            {
                Some(subagents_dir.join(&dirent.name))
            } else {
                None
            }
        }));
    }

    // Phase 2: OLD structure scan (fallback)
    let project_root = projects_dir.join(&base_dir);
    if let Ok(entries) = fs_provider.read_dir(&project_root) {
        for dirent in entries {
            if dirent.is_file
                && dirent.name.starts_with("agent-")
                && dirent.name.ends_with(".jsonl")
                && !dirent.name.contains("acompact")
            {
                // Skip if already found in NEW structure
                if files.iter().any(|f| {
                    f.file_name()
                        .map(|n| n == dirent.name.as_str())
                        .unwrap_or(false)
                }) {
                    continue;
                }

                let file_path = project_root.join(&dirent.name);
                if let Ok(content) = fs_provider.read_file_head(&file_path, 1) {
                    if subagent_belongs_to_session(&content, session_id) {
                        files.push(file_path);
                    }
                }
            }
        }
    }

    files
}

/// Check if a session has subagents.
pub(super) fn has_subagent_files(
    projects_dir: &PathBuf,
    fs_provider: &dyn crate::infrastructure::fs_provider::FsProvider,
    project_id: &str,
    session_id: &str,
) -> bool {
    !list_subagent_files(projects_dir, fs_provider, project_id, session_id).is_empty()
}

/// Parse a single subagent file.
pub(super) fn parse_subagent_file(
    fs_provider: &dyn crate::infrastructure::fs_provider::FsProvider,
    file_path: &Path,
) -> Option<super::Process> {
    let content = fs_provider.read_file(file_path).ok()?;
    let messages = parse_jsonl_content(&content);

    if messages.is_empty() {
        return None;
    }

    // Extract agent ID from filename
    let filename = file_path.file_name()?.to_str()?;
    let agent_id = filename
        .strip_prefix("agent-")?
        .strip_suffix(".jsonl")?
        .to_string();

    // Filter out compact files
    if agent_id.starts_with("acompact") {
        return None;
    }

    // Filter out warmup subagents
    if is_warmup_subagent(&messages) {
        return None;
    }

    // Calculate timing
    let (start_time_ms, end_time_ms, duration_ms) = calculate_timing(&messages);

    // Calculate metrics
    let metrics = calculate_metrics(&messages);

    // Check if ongoing
    let is_ongoing = check_is_ongoing(&messages);

    Some(super::Process {
        id: agent_id,
        file_path: file_path.to_string_lossy().to_string(),
        start_time_ms,
        end_time_ms,
        duration_ms,
        metrics,
        is_parallel: false,
        is_ongoing,
        task_id: None,
        messages,
        description: None,
        subagent_type: None,
        team: None,
    })
}

/// Check if this is a warmup subagent.
pub(super) fn is_warmup_subagent(messages: &[ParsedMessage]) -> bool {
    messages
        .iter()
        .find(|m| m.message_type == MessageType::User)
        .map(|m| m.content.as_str().unwrap_or("") == "Warmup")
        .unwrap_or(false)
}

/// Calculate timing from messages.
pub(super) fn calculate_timing(messages: &[ParsedMessage]) -> (u64, u64, u64) {
    let timestamps: Vec<u64> = messages
        .iter()
        .filter_map(|m| {
            chrono::DateTime::parse_from_rfc3339(&m.timestamp)
                .ok()
                .map(|dt| dt.timestamp_millis() as u64)
        })
        .collect();

    if timestamps.is_empty() {
        return (0, 0, 0);
    }

    let min_time = timestamps.iter().copied().min().unwrap_or(0);
    let max_time = timestamps.iter().copied().max().unwrap_or(0);

    (min_time, max_time, max_time.saturating_sub(min_time))
}

/// Calculate metrics from messages.
pub(super) fn calculate_metrics(messages: &[ParsedMessage]) -> SessionMetrics {
    let mut total_input = 0u64;
    let mut total_output = 0u64;
    let mut cache_read = 0u64;
    let mut cache_creation = 0u64;
    let mut message_count = 0u32;

    for msg in messages {
        message_count += 1;
        if let Some(ref usage) = msg.usage {
            total_input += usage.input_tokens;
            total_output += usage.output_tokens;
            cache_read += usage.cache_read_input_tokens.unwrap_or(0);
            cache_creation += usage.cache_creation_input_tokens.unwrap_or(0);
        }
    }

    SessionMetrics {
        duration_ms: 0,
        total_tokens: total_input + total_output,
        input_tokens: total_input,
        output_tokens: total_output,
        cache_read_tokens: if cache_read > 0 { Some(cache_read) } else { None },
        cache_creation_tokens: if cache_creation > 0 { Some(cache_creation) } else { None },
        message_count,
        cost_usd: None,
    }
}

/// Check if messages indicate ongoing session.
pub(super) fn check_is_ongoing(messages: &[ParsedMessage]) -> bool {
    crate::utils::session_state_detection::check_messages_ongoing(messages)
}

/// Detect parallel execution among subagents.
pub(super) fn detect_parallel_execution(subagents: &mut [super::Process]) {
    for i in 0..subagents.len() {
        for j in (i + 1)..subagents.len() {
            let a = &subagents[i];
            let b = &subagents[j];

            // Check if time windows overlap by more than PARALLEL_WINDOW_MS
            let overlap_start = a.start_time_ms.max(b.start_time_ms);
            let overlap_end = a.end_time_ms.min(b.end_time_ms);

            if overlap_end > overlap_start + PARALLEL_WINDOW_MS {
                subagents[i].is_parallel = true;
                subagents[j].is_parallel = true;
            }
        }
    }
}
