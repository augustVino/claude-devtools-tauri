//! SubagentResolver service - Links Task calls to subagent files and detects parallelism.
//!
//! Responsibilities:
//! - Find subagent JSONL files in {sessionId}/subagents/ directory
//! - Parse each subagent file
//! - Calculate start/end times and metrics
//! - Detect parallel execution (100ms overlap threshold)

use crate::infrastructure::fs_provider::FsProvider;
use crate::types::chunks::TeamInfo;
use crate::types::domain::SessionMetrics;
use crate::types::messages::{ParsedMessage, ToolCall};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    pub messages: Vec<SimpleMessage>,
    pub description: Option<String>,
    pub subagent_type: Option<String>,
    pub team: Option<crate::types::chunks::TeamInfo>,
}

/// SubagentResolver resolves subagent files and links them to Task calls.
pub struct SubagentResolver {
    projects_dir: PathBuf,
    fs_provider: Arc<dyn FsProvider>,
}

/// Parallel detection window in milliseconds
const PARALLEL_WINDOW_MS: u64 = 100;

/// Enrich a subagent Process with metadata from its parent Task call.
fn enrich_subagent_from_task(subagent: &mut Process, task_call: &ToolCall) {
    subagent.task_id = Some(task_call.id.clone());
    subagent.description = task_call.task_description.clone();
    subagent.subagent_type = task_call.task_subagent_type.clone();

    let team_name = task_call.input.get("team_name").and_then(|v| v.as_str());
    let member_name = task_call.input.get("name").and_then(|v| v.as_str());
    if let (Some(tn), Some(mn)) = (team_name, member_name) {
        subagent.team = Some(TeamInfo {
            team_name: tn.to_string(),
            member_name: mn.to_string(),
            member_color: String::new(),
        });
    }
}

/// Extract the summary attribute from a teammate-message tag in the first user message.
fn extract_team_message_summary(messages: &[SimpleMessage]) -> Option<String> {
    let first_user = messages.iter().find(|m| m.message_type == "user")?;
    let re = regex::Regex::new(r#"<teammate-message[^>]*\bsummary="([^"]+)""#).ok()?;
    re.captures(&first_user.content)
        .map(|cap| cap[1].to_string())
}

/// Link subagents to their parent Task calls using a 3-phase matching algorithm.
fn link_to_task_calls(
    subagents: &mut [Process],
    task_calls: &[ToolCall],
    messages: &[ParsedMessage],
) {
    // Phase 0: Preprocessing
    let task_calls_only: Vec<&ToolCall> = task_calls.iter().filter(|tc| tc.is_task).collect();
    if task_calls_only.is_empty() || subagents.is_empty() {
        return;
    }

    // Build agentId -> taskCallId mapping from tool results
    let mut agent_id_to_task_id: HashMap<String, String> = HashMap::new();
    for msg in messages {
        if let Some(result) = &msg.tool_use_result {
            let agent_id = result
                .get("agentId")
                .or_else(|| result.get("agent_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let task_call_id = msg.source_tool_use_id.clone()
                .or_else(|| msg.tool_results.first().map(|tr| tr.tool_use_id.clone()));
            if let (Some(aid), Some(tcid)) = (agent_id, task_call_id) {
                agent_id_to_task_id.insert(aid, tcid);
            }
        }
    }

    let task_call_by_id: HashMap<&str, &ToolCall> = task_calls_only
        .iter()
        .map(|tc| (tc.id.as_str(), *tc))
        .collect();

    let mut matched_subagent_ids: HashSet<String> = HashSet::new();
    let mut matched_task_ids: HashSet<String> = HashSet::new();

    // Phase 1: Result matching (agentId exact match)
    for subagent in subagents.iter_mut() {
        if let Some(task_call_id) = agent_id_to_task_id.get(&subagent.id) {
            if let Some(&task_call) = task_call_by_id.get(task_call_id.as_str()) {
                enrich_subagent_from_task(subagent, task_call);
                matched_subagent_ids.insert(subagent.id.clone());
                matched_task_ids.insert(task_call_id.clone());
            }
        }
    }

    // Phase 2: Description matching (team members)
    let team_task_calls: Vec<&&ToolCall> = task_calls_only
        .iter()
        .filter(|tc| {
            !matched_task_ids.contains(&tc.id)
                && tc.input.get("team_name").is_some()
                && tc.input.get("name").is_some()
        })
        .collect();

    if !team_task_calls.is_empty() {
        let mut subagent_summaries: HashMap<String, String> = HashMap::new();
        for subagent in subagents.iter() {
            if matched_subagent_ids.contains(&subagent.id) {
                continue;
            }
            if let Some(summary) = extract_team_message_summary(&subagent.messages) {
                subagent_summaries.insert(subagent.id.clone(), summary);
            }
        }

        for team_tc in &team_task_calls {
            let desc = match &team_tc.task_description {
                Some(d) if !d.is_empty() => d.clone(),
                _ => continue,
            };
            let mut best_match_idx: Option<usize> = None;
            let mut best_match_time: u64 = u64::MAX;
            for (i, subagent) in subagents.iter().enumerate() {
                if matched_subagent_ids.contains(&subagent.id) {
                    continue;
                }
                if subagent_summaries.get(&subagent.id).map(|s| s == &desc).unwrap_or(false) {
                    if subagent.start_time_ms < best_match_time {
                        best_match_time = subagent.start_time_ms;
                        best_match_idx = Some(i);
                    }
                }
            }
            if let Some(idx) = best_match_idx {
                enrich_subagent_from_task(&mut subagents[idx], team_tc);
                matched_subagent_ids.insert(subagents[idx].id.clone());
                matched_task_ids.insert(team_tc.id.clone());
            }
        }
    }

    // Phase 3: Positional fallback (no wrap-around)
    let mut unmatched_indices: Vec<usize> = subagents
        .iter()
        .enumerate()
        .filter(|(_, s)| !matched_subagent_ids.contains(&s.id))
        .map(|(i, _)| i)
        .collect();
    unmatched_indices.sort_by_key(|&i| subagents[i].start_time_ms);

    let unmatched_tasks: Vec<&&ToolCall> = task_calls_only
        .iter()
        .filter(|tc| !matched_task_ids.contains(&tc.id) && tc.input.get("team_name").is_none())
        .collect();

    let pair_count = unmatched_indices.len().min(unmatched_tasks.len());
    for i in 0..pair_count {
        enrich_subagent_from_task(&mut subagents[unmatched_indices[i]], unmatched_tasks[i]);
    }
}

impl SubagentResolver {
    /// Create a new SubagentResolver.
    pub fn new(projects_dir: PathBuf, fs_provider: Arc<dyn FsProvider>) -> Self {
        Self { projects_dir, fs_provider }
    }

    /// Resolve all subagents for a session.
    ///
    /// `task_calls` and `messages` are optional parent session data used for
    /// linking subagents to Task calls and enriching team metadata.
    pub fn resolve_subagents(
        &self,
        project_id: &str,
        session_id: &str,
        task_calls: Option<&[ToolCall]>,
        _messages: Option<&[ParsedMessage]>,
    ) -> Vec<Process> {
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

        if let (Some(tc), Some(msgs)) = (task_calls, _messages) {
            link_to_task_calls(&mut subagents, tc, msgs);
        }
        // TODO: propagate_team_metadata (Task 7)

        // Detect parallel execution
        self.detect_parallel_execution(&mut subagents);

        // TODO: enrich_team_colors (Task 8)

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

        if !self.fs_provider.exists(&subagents_dir).unwrap_or(false) {
            return Vec::new();
        }

        let entries = match self.fs_provider.read_dir(&subagents_dir) {
            Ok(entries) => entries,
            Err(_) => return Vec::new(),
        };

        entries
            .into_iter()
            .filter_map(|dirent| {
                if dirent.is_file && dirent.name.ends_with(".jsonl") {
                    if dirent.name.starts_with("agent-") && !dirent.name.contains("acompact") {
                        return Some(subagents_dir.join(&dirent.name));
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
        let content = self.fs_provider.read_file(file_path).ok()?;
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
            messages,
            description: None,
            subagent_type: None,
            team: None,
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
    uuid: Option<String>,
    parent_uuid: Option<String>,
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

            let uuid = json.get("uuid").and_then(|v| v.as_str()).map(|s| s.to_string());
            let parent_uuid = json.get("parentUuid").and_then(|v| v.as_str()).map(|s| s.to_string());

            Some(SimpleMessage {
                message_type,
                content,
                timestamp_ms,
                input_tokens,
                output_tokens,
                is_ongoing,
                uuid,
                parent_uuid,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::fs_provider::LocalFsProvider;
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn setup_test_env() -> (TempDir, SubagentResolver) {
        let temp_dir = TempDir::new().unwrap();
        let projects_dir = temp_dir.path().join("projects");
        fs::create_dir_all(&projects_dir).unwrap();

        let resolver = SubagentResolver::new(projects_dir, Arc::new(LocalFsProvider::new()));
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

        let subagents = resolver.resolve_subagents("-Users-test-project", "session-123", None, None);
        assert_eq!(subagents.len(), 1);
        assert_eq!(subagents[0].id, "test123");
    }

    #[test]
    fn test_parse_jsonl_lines_extracts_uuid_and_parent_uuid() {
        let content = r#"{"type":"user","message":"Hello","uuid":"abc-123","parentUuid":"parent-456","timestamp":"2024-01-01T00:00:00Z"}"#;
        let messages = parse_jsonl_lines(content);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].uuid, Some("abc-123".to_string()));
        assert_eq!(messages[0].parent_uuid, Some("parent-456".to_string()));
    }

    #[test]
    fn test_parse_jsonl_lines_missing_uuid() {
        let content = r#"{"type":"user","message":"Hello","timestamp":"2024-01-01T00:00:00Z"}"#;
        let messages = parse_jsonl_lines(content);
        assert_eq!(messages[0].uuid, None);
        assert_eq!(messages[0].parent_uuid, None);
    }

    // =========================================================================
    // Task 5: enrich_subagent_from_task + extract_team_message_summary
    // =========================================================================

    #[test]
    fn test_enrich_subagent_from_task_basic() {
        let mut process = Process {
            id: "agent-123".to_string(),
            file_path: "/tmp/test.jsonl".to_string(),
            start_time_ms: 1000,
            end_time_ms: 2000,
            duration_ms: 1000,
            metrics: SessionMetrics::default(),
            is_parallel: false,
            is_ongoing: false,
            task_id: None,
            messages: vec![],
            description: None,
            subagent_type: None,
            team: None,
        };
        let task_call = ToolCall {
            id: "tc-1".to_string(),
            name: "Task".to_string(),
            input: serde_json::json!({"prompt": "Fix the bug", "subagentType": "Explore"}),
            is_task: true,
            task_description: Some("Fix the bug".to_string()),
            task_subagent_type: Some("Explore".to_string()),
        };
        enrich_subagent_from_task(&mut process, &task_call);
        assert_eq!(process.task_id, Some("tc-1".to_string()));
        assert_eq!(process.description, Some("Fix the bug".to_string()));
        assert_eq!(process.subagent_type, Some("Explore".to_string()));
        assert!(process.team.is_none());
    }

    #[test]
    fn test_enrich_subagent_from_task_with_team() {
        let mut process = Process {
            id: "agent-456".to_string(),
            file_path: "/tmp/test.jsonl".to_string(),
            start_time_ms: 1000,
            end_time_ms: 2000,
            duration_ms: 1000,
            metrics: SessionMetrics::default(),
            is_parallel: false,
            is_ongoing: false,
            task_id: None,
            messages: vec![],
            description: None,
            subagent_type: None,
            team: None,
        };
        let task_call = ToolCall {
            id: "tc-2".to_string(),
            name: "Task".to_string(),
            input: serde_json::json!({"team_name": "my-team", "name": "researcher", "prompt": "Research X"}),
            is_task: true,
            task_description: Some("Research X".to_string()),
            task_subagent_type: None,
        };
        enrich_subagent_from_task(&mut process, &task_call);
        assert_eq!(process.task_id, Some("tc-2".to_string()));
        assert_eq!(process.description, Some("Research X".to_string()));
        let team = process.team.unwrap();
        assert_eq!(team.team_name, "my-team");
        assert_eq!(team.member_name, "researcher");
        assert_eq!(team.member_color, "");
    }

    #[test]
    fn test_extract_team_message_summary_found() {
        let messages = vec![SimpleMessage {
            message_type: "user".to_string(),
            content: r##"<teammate-message teammate_id="researcher@my-team" color="#FF5733" summary="Research topic X">message content</teammate-message>"##.to_string(),
            timestamp_ms: Some(1000),
            input_tokens: None,
            output_tokens: None,
            is_ongoing: None,
            uuid: Some("msg-1".to_string()),
            parent_uuid: Some("parent-1".to_string()),
        }];
        assert_eq!(
            extract_team_message_summary(&messages),
            Some("Research topic X".to_string())
        );
    }

    #[test]
    fn test_extract_team_message_summary_not_found() {
        let messages = vec![SimpleMessage {
            message_type: "user".to_string(),
            content: "Regular message".to_string(),
            timestamp_ms: Some(1000),
            input_tokens: None,
            output_tokens: None,
            is_ongoing: None,
            uuid: None,
            parent_uuid: None,
        }];
        assert_eq!(extract_team_message_summary(&messages), None);
    }

    #[test]
    fn test_extract_team_message_summary_empty() {
        let messages: Vec<SimpleMessage> = vec![];
        assert_eq!(extract_team_message_summary(&messages), None);
    }

    // =========================================================================
    // Task 6: link_to_task_calls
    // =========================================================================

    #[test]
    fn test_link_to_task_calls_phase1_agent_id_match() {
        let mut subagents = vec![Process {
            id: "agent-abc".to_string(),
            file_path: "/tmp/agent-abc.jsonl".to_string(),
            start_time_ms: 1000,
            end_time_ms: 2000,
            duration_ms: 1000,
            metrics: SessionMetrics::default(),
            is_parallel: false,
            is_ongoing: false,
            task_id: None,
            messages: vec![],
            description: None,
            subagent_type: None,
            team: None,
        }];
        let task_calls = vec![ToolCall {
            id: "tc-1".to_string(),
            name: "Task".to_string(),
            input: serde_json::json!({"prompt": "Explore code"}),
            is_task: true,
            task_description: Some("Explore code".to_string()),
            task_subagent_type: Some("Explore".to_string()),
        }];
        let messages = vec![ParsedMessage {
            tool_use_result: Some(serde_json::json!({"agentId": "agent-abc", "status": "done"})),
            source_tool_use_id: Some("tc-1".to_string()),
            ..Default::default()
        }];
        link_to_task_calls(&mut subagents, &task_calls, &messages);
        assert_eq!(subagents[0].task_id, Some("tc-1".to_string()));
        assert_eq!(subagents[0].description, Some("Explore code".to_string()));
        assert_eq!(subagents[0].subagent_type, Some("Explore".to_string()));
    }

    #[test]
    fn test_link_to_task_calls_phase2_team_description_match() {
        let mut subagents = vec![Process {
            id: "team-member".to_string(),
            file_path: "/tmp/team-member.jsonl".to_string(),
            start_time_ms: 1000,
            end_time_ms: 2000,
            duration_ms: 1000,
            metrics: SessionMetrics::default(),
            is_parallel: false,
            is_ongoing: false,
            task_id: None,
            messages: vec![SimpleMessage {
                message_type: "user".to_string(),
                content: r#"<teammate-message teammate_id="researcher@my-team" summary="Research X">msg</teammate-message>"#.to_string(),
                timestamp_ms: Some(1000),
                input_tokens: None,
                output_tokens: None,
                is_ongoing: None,
                uuid: None,
                parent_uuid: None,
            }],
            description: None,
            subagent_type: None,
            team: None,
        }];
        let task_calls = vec![ToolCall {
            id: "tc-team".to_string(),
            name: "Task".to_string(),
            input: serde_json::json!({"team_name": "my-team", "name": "researcher", "prompt": "Research X"}),
            is_task: true,
            task_description: Some("Research X".to_string()),
            task_subagent_type: None,
        }];
        let messages: Vec<ParsedMessage> = vec![];
        link_to_task_calls(&mut subagents, &task_calls, &messages);
        assert_eq!(subagents[0].task_id, Some("tc-team".to_string()));
        assert_eq!(
            subagents[0].team.as_ref().unwrap().team_name,
            "my-team"
        );
    }

    #[test]
    fn test_link_to_task_calls_phase3_positional_fallback() {
        let mut subagents = vec![
            Process {
                id: "agent-1".to_string(),
                file_path: "/tmp/a1.jsonl".to_string(),
                start_time_ms: 1000,
                end_time_ms: 2000,
                duration_ms: 1000,
                metrics: SessionMetrics::default(),
                is_parallel: false,
                is_ongoing: false,
                task_id: None,
                messages: vec![],
                description: None,
                subagent_type: None,
                team: None,
            },
            Process {
                id: "agent-2".to_string(),
                file_path: "/tmp/a2.jsonl".to_string(),
                start_time_ms: 2000,
                end_time_ms: 3000,
                duration_ms: 1000,
                metrics: SessionMetrics::default(),
                is_parallel: false,
                is_ongoing: false,
                task_id: None,
                messages: vec![],
                description: None,
                subagent_type: None,
                team: None,
            },
        ];
        let task_calls = vec![
            ToolCall {
                id: "tc-1".to_string(),
                name: "Task".to_string(),
                input: serde_json::json!({"prompt": "First task"}),
                is_task: true,
                task_description: Some("First task".to_string()),
                task_subagent_type: None,
            },
            ToolCall {
                id: "tc-2".to_string(),
                name: "Task".to_string(),
                input: serde_json::json!({"prompt": "Second task"}),
                is_task: true,
                task_description: Some("Second task".to_string()),
                task_subagent_type: None,
            },
        ];
        let messages: Vec<ParsedMessage> = vec![];
        link_to_task_calls(&mut subagents, &task_calls, &messages);
        assert_eq!(subagents[0].description, Some("First task".to_string()));
        assert_eq!(subagents[1].description, Some("Second task".to_string()));
    }
}