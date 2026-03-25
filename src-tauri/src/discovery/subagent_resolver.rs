//! SubagentResolver service - Links Task calls to subagent files and detects parallelism.
//!
//! Responsibilities:
//! - Find subagent JSONL files in {sessionId}/subagents/ directory
//! - Parse each subagent file
//! - Calculate start/end times and metrics
//! - Detect parallel execution (100ms overlap threshold)

use crate::types::domain::SessionMetrics;
use std::fs;
use std::path::{Path, PathBuf};

/// Process represents a subagent execution.
#[derive(Debug, Clone)]
pub struct Process {
    pub id: String,
    pub file_path: String,
    pub start_time_ms: u64,
    pub end_time_ms: u64,
    pub duration_ms: u64,
    pub metrics: SessionMetrics,
    pub is_parallel: bool,
    pub is_ongoing: bool,
    pub task_id: Option<String>,
}

/// SubagentResolver resolves subagent files and links them to Task calls.
pub struct SubagentResolver {
    projects_dir: PathBuf,
}

/// Parallel detection window in milliseconds
const PARALLEL_WINDOW_MS: u64 = 100;

impl SubagentResolver {
    /// Create a new SubagentResolver.
    pub fn new(projects_dir: PathBuf) -> Self {
        Self { projects_dir }
    }

    /// Resolve all subagents for a session.
    pub fn resolve_subagents(&self, project_id: &str, session_id: &str) -> Vec<Process> {
        // Get subagent files
        let subagent_files = self.list_subagent_files(project_id, session_id);

        if subagent_files.is_empty() {
            return Vec::new();
        }

        // Parse subagent files
        let mut subagents: Vec<Process> = subagent_files
            .into_iter()
            .filter_map(|file_path| self.parse_subagent_file(&file_path))
            .collect();

        // Detect parallel execution
        self.detect_parallel_execution(&mut subagents);

        // Sort by start time
        subagents.sort_by_key(|s| s.start_time_ms);

        subagents
    }

    /// List subagent files for a session.
    pub fn list_subagent_files(&self, project_id: &str, session_id: &str) -> Vec<PathBuf> {
        let base_dir = crate::utils::path_decoder::extract_base_dir(project_id);
        let subagents_dir = self.projects_dir
            .join(&base_dir)
            .join(session_id)
            .join("subagents");

        if !subagents_dir.exists() {
            return Vec::new();
        }

        let entries = match fs::read_dir(&subagents_dir) {
            Ok(entries) => entries,
            Err(_) => return Vec::new(),
        };

        entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if path.is_file() && path.extension().map_or(false, |ext| ext == "jsonl") {
                    let filename = path.file_name()?.to_str()?;
                    if filename.starts_with("agent-") && !filename.contains("acompact") {
                        return Some(path);
                    }
                }
                None
            })
            .collect()
    }

    /// Check if a session has subagents.
    pub fn has_subagents(&self, project_id: &str, session_id: &str) -> bool {
        !self.list_subagent_files(project_id, session_id).is_empty()
    }

    /// Parse a single subagent file.
    fn parse_subagent_file(&self, file_path: &Path) -> Option<Process> {
        let content = fs::read_to_string(file_path).ok()?;
        let messages = parse_jsonl_lines(&content);

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
        if self.is_warmup_subagent(&messages) {
            return None;
        }

        // Calculate timing
        let (start_time_ms, end_time_ms, duration_ms) = self.calculate_timing(&messages);

        // Calculate metrics
        let metrics = self.calculate_metrics(&messages);

        // Check if ongoing
        let is_ongoing = self.check_is_ongoing(&messages);

        Some(Process {
            id: agent_id,
            file_path: file_path.to_string_lossy().to_string(),
            start_time_ms,
            end_time_ms,
            duration_ms,
            metrics,
            is_parallel: false,
            is_ongoing,
            task_id: None,
        })
    }

    /// Check if this is a warmup subagent.
    fn is_warmup_subagent(&self, messages: &[SimpleMessage]) -> bool {
        messages
            .iter()
            .find(|m| m.message_type == "user")
            .map(|m| m.content.as_str() == "Warmup")
            .unwrap_or(false)
    }

    /// Calculate timing from messages.
    fn calculate_timing(&self, messages: &[SimpleMessage]) -> (u64, u64, u64) {
        let timestamps: Vec<u64> = messages
            .iter()
            .filter_map(|m| m.timestamp_ms)
            .collect();

        if timestamps.is_empty() {
            return (0, 0, 0);
        }

        let min_time = timestamps.iter().copied().min().unwrap_or(0);
        let max_time = timestamps.iter().copied().max().unwrap_or(0);

        (min_time, max_time, max_time.saturating_sub(min_time))
    }

    /// Calculate metrics from messages.
    fn calculate_metrics(&self, messages: &[SimpleMessage]) -> SessionMetrics {
        let mut total_input = 0u64;
        let mut total_output = 0u64;
        let mut message_count = 0u32;

        for msg in messages {
            message_count += 1;
            total_input += msg.input_tokens.unwrap_or(0);
            total_output += msg.output_tokens.unwrap_or(0);
        }

        SessionMetrics {
            duration_ms: 0,
            total_tokens: total_input + total_output,
            input_tokens: total_input,
            output_tokens: total_output,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            message_count,
            cost_usd: None,
        }
    }

    /// Check if messages indicate ongoing session.
    fn check_is_ongoing(&self, messages: &[SimpleMessage]) -> bool {
        // Simple check: last message should not be a result with end status
        if let Some(last) = messages.last() {
            last.message_type == "assistant" && last.is_ongoing.unwrap_or(false)
        } else {
            false
        }
    }

    /// Detect parallel execution among subagents.
    fn detect_parallel_execution(&self, subagents: &mut [Process]) {
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
}

/// Simple message representation for parsing.
#[derive(Debug, Clone)]
struct SimpleMessage {
    message_type: String,
    content: String,
    timestamp_ms: Option<u64>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    is_ongoing: Option<bool>,
}

/// Parse JSONL lines into simple messages.
fn parse_jsonl_lines(content: &str) -> Vec<SimpleMessage> {
    content
        .lines()
        .filter_map(|line| {
            let json: serde_json::Value = serde_json::from_str(line).ok()?;
            let message_type = json.get("type")?.as_str()?.to_string();

            let content = match message_type.as_str() {
                "user" => json.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string(),
                "assistant" => {
                    if let Some(content_arr) = json.get("content").and_then(|c| c.as_array()) {
                        content_arr
                            .iter()
                            .filter_map(|item| {
                                if item.get("type")?.as_str()? == "text" {
                                    item.get("text").and_then(|t| t.as_str())
                                } else {
                                    None
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    } else {
                        String::new()
                    }
                }
                _ => String::new(),
            };

            let timestamp_ms = json
                .get("timestamp")
                .and_then(|t| t.as_str())
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .map(|dt| dt.timestamp_millis() as u64);

            let usage = json.get("usage");
            let input_tokens = usage.and_then(|u| u.get("input_tokens")).and_then(|t| t.as_u64());
            let output_tokens = usage.and_then(|u| u.get("output_tokens")).and_then(|t| t.as_u64());

            let is_ongoing = json.get("is_ongoing").and_then(|v| v.as_bool());

            Some(SimpleMessage {
                message_type,
                content,
                timestamp_ms,
                input_tokens,
                output_tokens,
                is_ongoing,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_env() -> (TempDir, SubagentResolver) {
        let temp_dir = TempDir::new().unwrap();
        let projects_dir = temp_dir.path().join("projects");
        fs::create_dir_all(&projects_dir).unwrap();

        let resolver = SubagentResolver::new(projects_dir);
        (temp_dir, resolver)
    }

    #[test]
    fn test_list_subagent_files_empty() {
        let (_temp_dir, resolver) = setup_test_env();
        let files = resolver.list_subagent_files("-Users-test-project", "session-123");
        assert!(files.is_empty());
    }

    #[test]
    fn test_has_subagents_false() {
        let (_temp_dir, resolver) = setup_test_env();
        assert!(!resolver.has_subagents("-Users-test-project", "session-123"));
    }

    #[test]
    fn test_list_subagent_files_with_files() {
        let (temp_dir, resolver) = setup_test_env();

        // Create subagent directory
        let subagents_dir = temp_dir.path()
            .join("projects")
            .join("-Users-test-project")
            .join("session-123")
            .join("subagents");
        fs::create_dir_all(&subagents_dir).unwrap();

        // Create subagent file
        fs::write(
            subagents_dir.join("agent-test123.jsonl"),
            r#"{"type":"user","message":"Hello"}"#,
        )
        .unwrap();

        let files = resolver.list_subagent_files("-Users-test-project", "session-123");
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn test_resolve_subagents() {
        let (temp_dir, resolver) = setup_test_env();

        // Create subagent directory
        let subagents_dir = temp_dir.path()
            .join("projects")
            .join("-Users-test-project")
            .join("session-123")
            .join("subagents");
        fs::create_dir_all(&subagents_dir).unwrap();

        // Create subagent file
        fs::write(
            subagents_dir.join("agent-test123.jsonl"),
            r#"{"type":"user","message":"Hello","timestamp":"2024-01-01T00:00:00Z"}
{"type":"assistant","content":[{"type":"text","text":"Response"}],"timestamp":"2024-01-01T00:01:00Z"}"#,
        )
        .unwrap();

        let subagents = resolver.resolve_subagents("-Users-test-project", "session-123");
        assert_eq!(subagents.len(), 1);
        assert_eq!(subagents[0].id, "test123");
    }
}