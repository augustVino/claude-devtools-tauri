//! SubagentResolver service - Links Task calls to subagent files and detects parallelism.
//!
//! Responsibilities:
//! - Find subagent JSONL files in {sessionId}/subagents/ directory
//! - Parse each subagent file
//! - Calculate start/end times and metrics
//! - Detect parallel execution (100ms overlap threshold)

use crate::infrastructure::fs_provider::FsProvider;
use crate::parsing::jsonl_parser::parse_jsonl_content;
use crate::types::chunks::TeamInfo;
use crate::types::domain::{MessageType, SessionMetrics};
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
    pub messages: Vec<ParsedMessage>,
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

/// Maximum depth for parentUuid chain traversal when propagating team metadata.
const MAX_PARENT_DEPTH: usize = 10;

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
fn extract_team_message_summary(messages: &[ParsedMessage]) -> Option<String> {
    let first_user = messages.iter().find(|m| m.message_type == MessageType::User)?;
    let content_str = first_user.content.as_str().unwrap_or("");
    let re = regex::Regex::new(r#"<teammate-message[^>]*\bsummary="([^"]+)""#).ok()?;
    re.captures(content_str)
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

/// Propagate team metadata to continuation files via parentUuid chain.
fn propagate_team_metadata(subagents: &mut [Process]) {
    // Build last message uuid -> subagent index mapping
    let mut last_uuid_to_idx: HashMap<String, usize> = HashMap::new();
    for (i, subagent) in subagents.iter().enumerate() {
        if let Some(last) = subagent.messages.last() {
            if !last.uuid.is_empty() {
                last_uuid_to_idx.insert(last.uuid.clone(), i);
            }
        }
    }

    // Phase 1: Collect which subagent each continuation should inherit from
    let mut inherit_from: Vec<Option<usize>> = vec![None; subagents.len()];
    for (i, subagent) in subagents.iter().enumerate() {
        if subagent.team.is_some() {
            continue;
        }
        if subagent.messages.is_empty() {
            continue;
        }

        let first_parent_uuid = match subagent.messages.first().and_then(|m| m.parent_uuid.as_ref()) {
            Some(uuid) if !uuid.is_empty() => uuid.clone(),
            _ => continue,
        };

        // Walk parentUuid chain
        let mut current_uuid = first_parent_uuid;
        let mut depth = 0;
        let mut ancestor_idx: Option<usize> = None;

        while depth < MAX_PARENT_DEPTH {
            if let Some(&idx) = last_uuid_to_idx.get(&current_uuid) {
                if subagents[idx].team.is_some() {
                    ancestor_idx = Some(idx);
                    break;
                }
                if let Some(prev_last) = subagents[idx].messages.last() {
                    if let Some(prev_parent) = &prev_last.parent_uuid {
                        current_uuid = prev_parent.clone();
                        depth += 1;
                        continue;
                    }
                }
            }
            break;
        }

        inherit_from[i] = ancestor_idx;
    }

    // Phase 2: Apply inheritance
    // Collect cloned data to avoid simultaneous borrow of different indices in the slice
    let inherited: Vec<(usize, Option<TeamInfo>, Option<String>, Option<String>, Option<String>)> = inherit_from
        .iter()
        .enumerate()
        .filter_map(|(i, anc)| {
            let anc = (*anc)?;
            let ancestor = &subagents[anc];
            Some((
                i,
                ancestor.team.clone(),
                ancestor.task_id.clone(),
                ancestor.description.clone(),
                ancestor.subagent_type.clone(),
            ))
        })
        .collect();

    for (i, team, task_id, description, subagent_type) in inherited {
        subagents[i].team = team;
        subagents[i].task_id = subagents[i].task_id.take().or(task_id);
        subagents[i].description = subagents[i].description.take().or(description);
        subagents[i].subagent_type = subagents[i].subagent_type.take().or(subagent_type);
    }
}

/// Inject team member colors from teammate_spawned tool results.
fn enrich_team_colors(subagents: &mut [Process], messages: &[ParsedMessage]) {
    for msg in messages {
        let source_id = match &msg.source_tool_use_id {
            Some(id) if !id.is_empty() => id.as_str(),
            _ => continue,
        };
        let result = match &msg.tool_use_result {
            Some(r) => r,
            None => continue,
        };
        let status = match result.get("status").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => continue,
        };
        let color = match result.get("color").and_then(|v| v.as_str()) {
            Some(c) if !c.is_empty() => c.to_string(),
            _ => continue,
        };
        if status != "teammate_spawned" {
            continue;
        }
        for subagent in subagents.iter_mut() {
            if subagent.task_id.as_deref() == Some(source_id) {
                if let Some(team) = &mut subagent.team {
                    team.member_color = color.clone();
                }
            }
        }
    }
}

/// Check if a JSONL file belongs to a specific session (for OLD directory structure).
fn subagent_belongs_to_session(file_content: &str, target_session_id: &str) -> bool {
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
        messages: Option<&[ParsedMessage]>,
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

        if let (Some(tc), Some(msgs)) = (task_calls, messages) {
            link_to_task_calls(&mut subagents, tc, msgs);
        }

        propagate_team_metadata(&mut subagents);

        // Detect parallel execution
        self.detect_parallel_execution(&mut subagents);

        if let Some(msgs) = messages {
            enrich_team_colors(&mut subagents, msgs);
        }

        // Sort by start time
        subagents.sort_by_key(|s| s.start_time_ms);

        subagents
    }

    /// List subagent files for a session.
    ///
    /// Scans two directory structures:
    /// - **Phase 1 (NEW)**: `{projectId}/{sessionId}/subagents/agent-{id}.jsonl`
    /// - **Phase 2 (OLD)**: `{projectId}/agent-{id}.jsonl` (matched by sessionId in first line)
    pub fn list_subagent_files(&self, project_id: &str, session_id: &str) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let base_dir = crate::utils::path_decoder::extract_base_dir(project_id);

        // Phase 1: NEW structure scan
        let subagents_dir = self.projects_dir
            .join(&base_dir)
            .join(session_id)
            .join("subagents");

        if let Ok(entries) = self.fs_provider.read_dir(&subagents_dir) {
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
        let project_root = self.projects_dir.join(&base_dir);
        if let Ok(entries) = self.fs_provider.read_dir(&project_root) {
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
                    if let Ok(content) = self.fs_provider.read_file_head(&file_path, 1) {
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
    pub fn has_subagents(&self, project_id: &str, session_id: &str) -> bool {
        !self.list_subagent_files(project_id, session_id).is_empty()
    }

    /// Parse a single subagent file.
    fn parse_subagent_file(&self, file_path: &Path) -> Option<Process> {
        let content = self.fs_provider.read_file(file_path).ok()?;
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
    fn is_warmup_subagent(&self, messages: &[ParsedMessage]) -> bool {
        messages
            .iter()
            .find(|m| m.message_type == MessageType::User)
            .map(|m| m.content.as_str().unwrap_or("") == "Warmup")
            .unwrap_or(false)
    }

    /// Calculate timing from messages.
    fn calculate_timing(&self, messages: &[ParsedMessage]) -> (u64, u64, u64) {
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
    fn calculate_metrics(&self, messages: &[ParsedMessage]) -> SessionMetrics {
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
    fn check_is_ongoing(&self, messages: &[ParsedMessage]) -> bool {
        crate::utils::session_state_detection::check_messages_ongoing(messages)
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

    /// Find a subagent by ID.
    pub fn find_subagent_by_id<'a>(subagents: &'a [Process], id: &str) -> Option<&'a Process> {
        subagents.iter().find(|s| s.id == id)
    }

    /// Get aggregated metrics across all subagents.
    pub fn get_total_subagent_metrics(subagents: &[Process]) -> SessionMetrics {
        if subagents.is_empty() {
            return SessionMetrics::default();
        }

        let mut total_input = 0u64;
        let mut total_output = 0u64;
        let mut total_cache_read: Option<u64> = Some(0);
        let mut total_cache_creation: Option<u64> = Some(0);
        let mut total_messages = 0u32;

        for s in subagents {
            total_input += s.metrics.input_tokens;
            total_output += s.metrics.output_tokens;
            total_messages += s.metrics.message_count;
            total_cache_read =
                Some(total_cache_read.unwrap_or(0) + s.metrics.cache_read_tokens.unwrap_or(0));
            total_cache_creation =
                Some(total_cache_creation.unwrap_or(0) + s.metrics.cache_creation_tokens.unwrap_or(0));
        }

        SessionMetrics {
            duration_ms: 0,
            total_tokens: total_input + total_output,
            input_tokens: total_input,
            output_tokens: total_output,
            cache_read_tokens: total_cache_read,
            cache_creation_tokens: total_cache_creation,
            message_count: total_messages,
            cost_usd: None,
        }
    }
}

/// Helper to create a minimal ParsedMessage for tests.
fn make_test_message(msg_type: MessageType, content: &str, uuid: &str, parent_uuid: Option<&str>, timestamp: &str) -> ParsedMessage {
    ParsedMessage {
        message_type: msg_type,
        content: serde_json::Value::String(content.to_string()),
        uuid: uuid.to_string(),
        parent_uuid: parent_uuid.map(|s| s.to_string()),
        timestamp: timestamp.to_string(),
        ..Default::default()
    }
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
            r#"{"type":"user","message":{"role":"user","content":"Hello"},"uuid":"u1","timestamp":"2024-01-01T00:00:00Z"}
{"type":"assistant","content":[{"type":"text","text":"Response"}],"uuid":"u2","timestamp":"2024-01-01T00:01:00Z"}"#,
        )
        .unwrap();

        let subagents = resolver.resolve_subagents("-Users-test-project", "session-123", None, None);
        assert_eq!(subagents.len(), 1);
        assert_eq!(subagents[0].id, "test123");
    }

    #[test]
    fn test_parse_jsonl_content_extracts_uuid_and_parent_uuid() {
        let content = r#"{"type":"user","message":{"role":"user","content":"Hello"},"uuid":"abc-123","parentUuid":"parent-456","timestamp":"2024-01-01T00:00:00Z"}"#;
        let messages = parse_jsonl_content(content);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].uuid, "abc-123");
        assert_eq!(messages[0].parent_uuid, Some("parent-456".to_string()));
    }

    #[test]
    fn test_parse_jsonl_content_missing_uuid() {
        // parse_jsonl_line skips messages with empty uuid, so this returns empty
        let content = r#"{"type":"user","message":{"role":"user","content":"Hello"},"timestamp":"2024-01-01T00:00:00Z"}"#;
        let messages = parse_jsonl_content(content);
        assert!(messages.is_empty());
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
        let messages = vec![make_test_message(
            MessageType::User,
            r##"<teammate-message teammate_id="researcher@my-team" color="#FF5733" summary="Research topic X">message content</teammate-message>"##,
            "msg-1",
            Some("parent-1"),
            "2026-01-01T00:00:00+00:00",
        )];
        assert_eq!(
            extract_team_message_summary(&messages),
            Some("Research topic X".to_string())
        );
    }

    #[test]
    fn test_extract_team_message_summary_not_found() {
        let messages = vec![make_test_message(
            MessageType::User,
            "Regular message",
            "msg-1",
            None,
            "2026-01-01T00:00:00+00:00",
        )];
        assert_eq!(extract_team_message_summary(&messages), None);
    }

    #[test]
    fn test_extract_team_message_summary_empty() {
        let messages: Vec<ParsedMessage> = vec![];
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
            messages: vec![make_test_message(
                MessageType::User,
                r#"<teammate-message teammate_id="researcher@my-team" summary="Research X">msg</teammate-message>"#,
                "msg-1",
                None,
                "2026-01-01T00:00:00+00:00",
            )],
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

    // =========================================================================
    // Task 7: propagate_team_metadata
    // =========================================================================

    #[test]
    fn test_propagate_team_metadata_chain() {
        let mut subagents = vec![
            Process {
                id: "agent-main".to_string(), file_path: "/tmp/main.jsonl".to_string(),
                start_time_ms: 1000, end_time_ms: 2000, duration_ms: 1000,
                metrics: SessionMetrics::default(), is_parallel: false, is_ongoing: false,
                task_id: Some("tc-1".to_string()),
                messages: vec![
                    make_test_message(MessageType::User, "msg", "main-uuid", None, "2026-01-01T00:00:01+00:00"),
                    make_test_message(MessageType::Assistant, "resp", "main-last", None, "2026-01-01T00:00:02+00:00"),
                ],
                description: Some("Main task".to_string()), subagent_type: Some("Explore".to_string()),
                team: Some(TeamInfo {
                    team_name: "my-team".to_string(), member_name: "researcher".to_string(),
                    member_color: "#FF5733".to_string(),
                }),
            },
            Process {
                id: "agent-cont".to_string(), file_path: "/tmp/cont.jsonl".to_string(),
                start_time_ms: 3000, end_time_ms: 4000, duration_ms: 1000,
                metrics: SessionMetrics::default(), is_parallel: false, is_ongoing: false,
                task_id: None,
                messages: vec![
                    make_test_message(MessageType::User, "cont msg", "cont-uuid", Some("main-last"), "2026-01-01T00:00:03+00:00"),
                    make_test_message(MessageType::Assistant, "cont resp", "cont-last", None, "2026-01-01T00:00:04+00:00"),
                ],
                description: None, subagent_type: None, team: None,
            },
        ];

        propagate_team_metadata(&mut subagents);

        assert!(subagents[1].team.is_some());
        let team = subagents[1].team.as_ref().unwrap();
        assert_eq!(team.team_name, "my-team");
        assert_eq!(team.member_name, "researcher");
        assert_eq!(subagents[1].description, Some("Main task".to_string()));
        assert_eq!(subagents[1].subagent_type, Some("Explore".to_string()));
    }

    // =========================================================================
    // Task 11: OLD directory structure scanning
    // =========================================================================

    #[test]
    fn test_list_subagent_files_old_structure() {
        let (temp_dir, resolver) = setup_test_env();

        // Create OLD structure: agent file in project root (no session subdirectory)
        let old_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&old_dir).unwrap();

        // File with sessionId matching in first line
        fs::write(
            old_dir.join("agent-old123.jsonl"),
            r#"{"type":"user","message":"Hello","sessionId":"session-456","uuid":"u1"}"#,
        )
        .unwrap();

        // Also a file for a different session (should be filtered out)
        fs::write(
            old_dir.join("agent-other.jsonl"),
            r#"{"type":"user","message":"Other","sessionId":"session-999","uuid":"u2"}"#,
        )
        .unwrap();

        let files = resolver.list_subagent_files("-Users-test-project", "session-456");
        assert_eq!(files.len(), 1);
        assert!(files[0].to_string_lossy().contains("agent-old123.jsonl"));
    }

    #[test]
    fn test_list_subagent_files_old_and_new_combined() {
        let (temp_dir, resolver) = setup_test_env();

        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");

        // NEW structure file
        let subagents_dir = project_dir.join("session-456").join("subagents");
        fs::create_dir_all(&subagents_dir).unwrap();
        fs::write(
            subagents_dir.join("agent-new1.jsonl"),
            r#"{"type":"user","message":"New"}"#,
        )
        .unwrap();

        // OLD structure file (different name, matching sessionId)
        fs::write(
            project_dir.join("agent-old123.jsonl"),
            r#"{"type":"user","message":"Old","sessionId":"session-456","uuid":"u1"}"#,
        )
        .unwrap();

        // OLD structure file (different session, should be excluded)
        fs::write(
            project_dir.join("agent-other.jsonl"),
            r#"{"type":"user","message":"Other","sessionId":"session-999","uuid":"u2"}"#,
        )
        .unwrap();

        let files = resolver.list_subagent_files("-Users-test-project", "session-456");
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_subagent_belongs_to_session() {
        // Matching sessionId
        assert!(subagent_belongs_to_session(
            r#"{"type":"user","sessionId":"session-456"}"#,
            "session-456"
        ));

        // Non-matching sessionId
        assert!(!subagent_belongs_to_session(
            r#"{"type":"user","sessionId":"session-999"}"#,
            "session-456"
        ));

        // Missing sessionId field
        assert!(!subagent_belongs_to_session(
            r#"{"type":"user","message":"hello"}"#,
            "session-456"
        ));

        // Invalid JSON
        assert!(!subagent_belongs_to_session(
            "not valid json",
            "session-456"
        ));

        // Empty content
        assert!(!subagent_belongs_to_session("", "session-456"));
    }

    #[test]
    fn test_list_subagent_files_old_structure_skips_acompact() {
        let (temp_dir, resolver) = setup_test_env();

        let old_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&old_dir).unwrap();

        fs::write(
            old_dir.join("agent-acompact-abc.jsonl"),
            r#"{"type":"user","sessionId":"session-456"}"#,
        )
        .unwrap();

        let files = resolver.list_subagent_files("-Users-test-project", "session-456");
        assert!(files.is_empty());
    }

    // =========================================================================
    // Task 8: enrich_team_colors
    // =========================================================================

    #[test]
    fn test_enrich_team_colors_from_teammate_spawned() {
        use crate::types::messages::ParsedMessage;

        let mut subagents = vec![Process {
            id: "agent-1".to_string(), file_path: "/tmp/a1.jsonl".to_string(),
            start_time_ms: 1000, end_time_ms: 2000, duration_ms: 1000,
            metrics: SessionMetrics::default(), is_parallel: false, is_ongoing: false,
            task_id: Some("tc-1".to_string()), messages: vec![], description: None, subagent_type: None,
            team: Some(TeamInfo {
                team_name: "my-team".to_string(), member_name: "researcher".to_string(),
                member_color: String::new(),
            }),
        }];
        let messages = vec![ParsedMessage {
            tool_use_result: Some(serde_json::json!({
                "status": "teammate_spawned",
                "color": "#FF5733"
            })),
            source_tool_use_id: Some("tc-1".to_string()),
            ..Default::default()
        }];
        enrich_team_colors(&mut subagents, &messages);
        assert_eq!(subagents[0].team.as_ref().unwrap().member_color, "#FF5733");
    }

    // =========================================================================
    // Task 12: find_subagent_by_id and get_total_subagent_metrics helpers
    // =========================================================================

    #[test]
    fn test_find_subagent_by_id_found() {
        let subagents = vec![Process {
            id: "abc".to_string(),
            file_path: "/tmp/a.jsonl".to_string(),
            start_time_ms: 0,
            end_time_ms: 0,
            duration_ms: 0,
            metrics: SessionMetrics::default(),
            is_parallel: false,
            is_ongoing: false,
            task_id: None,
            messages: vec![],
            description: None,
            subagent_type: None,
            team: None,
        }];
        assert!(SubagentResolver::find_subagent_by_id(&subagents, "abc").is_some());
        assert!(SubagentResolver::find_subagent_by_id(&subagents, "xyz").is_none());
    }

    #[test]
    fn test_get_total_subagent_metrics() {
        let subagents = vec![
            Process {
                id: "a".to_string(),
                file_path: "/tmp/a.jsonl".to_string(),
                start_time_ms: 0,
                end_time_ms: 1000,
                duration_ms: 1000,
                metrics: SessionMetrics {
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_read_tokens: Some(20),
                    cache_creation_tokens: Some(10),
                    message_count: 3,
                    ..Default::default()
                },
                is_parallel: false,
                is_ongoing: false,
                task_id: None,
                messages: vec![],
                description: None,
                subagent_type: None,
                team: None,
            },
            Process {
                id: "b".to_string(),
                file_path: "/tmp/b.jsonl".to_string(),
                start_time_ms: 0,
                end_time_ms: 2000,
                duration_ms: 2000,
                metrics: SessionMetrics {
                    input_tokens: 200,
                    output_tokens: 100,
                    cache_read_tokens: Some(30),
                    cache_creation_tokens: Some(5),
                    message_count: 5,
                    ..Default::default()
                },
                is_parallel: false,
                is_ongoing: false,
                task_id: None,
                messages: vec![],
                description: None,
                subagent_type: None,
                team: None,
            },
        ];
        let total = SubagentResolver::get_total_subagent_metrics(&subagents);
        assert_eq!(total.input_tokens, 300);
        assert_eq!(total.output_tokens, 150);
        assert_eq!(total.cache_read_tokens, Some(50));
        assert_eq!(total.cache_creation_tokens, Some(15));
        assert_eq!(total.message_count, 8);
    }

    #[test]
    fn test_get_total_subagent_metrics_empty() {
        let total = SubagentResolver::get_total_subagent_metrics(&[]);
        assert_eq!(total.input_tokens, 0);
    }
}