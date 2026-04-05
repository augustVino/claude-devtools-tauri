//! SubagentResolver service - Links Task calls to subagent files and detects parallelism.
//!
//! Responsibilities:
//! - Find subagent JSONL files in {sessionId}/subagents/ directory
//! - Parse each subagent file
//! - Calculate start/end times and metrics
//! - Detect parallel execution (100ms overlap threshold)
//!
//! This module contains the core structs and orchestration logic.
//! Identity/linking functions are in sibling modules:
//! - `subagent_identity` - file listing, parsing, timing, metrics, parallel detection
//! - `subagent_linking` - task call matching, team metadata propagation, color enrichment

// Import sibling modules (declared in discovery/mod.rs)
#[allow(unused_imports)]
use crate::discovery::subagent_identity::{
    detect_parallel_execution, has_subagent_files, list_subagent_files, parse_subagent_file,
    subagent_belongs_to_session,
};
use crate::discovery::subagent_linking::{
    enrich_team_colors, link_to_task_calls, propagate_team_metadata,
};

use crate::infrastructure::fs_provider::FsProvider;
#[allow(unused_imports)]
use crate::types::domain::{MessageType, SessionMetrics};
use crate::types::messages::{ParsedMessage, ToolCall};
use std::path::PathBuf;
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
        let subagent_files = list_subagent_files(&self.projects_dir, self.fs_provider.as_ref(), project_id, session_id);

        if subagent_files.is_empty() {
            return Vec::new();
        }

        // Parse subagent files
        let mut subagents: Vec<Process> = subagent_files
            .into_iter()
            .filter_map(|file_path| parse_subagent_file(self.fs_provider.as_ref(), &file_path))
            .collect();

        if let (Some(tc), Some(msgs)) = (task_calls, messages) {
            link_to_task_calls(&mut subagents, tc, msgs);
        }

        propagate_team_metadata(&mut subagents);

        // Detect parallel execution
        detect_parallel_execution(&mut subagents);

        if let Some(msgs) = messages {
            enrich_team_colors(&mut subagents, msgs);
        }

        // Sort by start time
        subagents.sort_by_key(|s| s.start_time_ms);

        subagents
    }

    /// Check if a session has subagents.
    #[allow(dead_code)]
    pub fn has_subagents(&self, project_id: &str, session_id: &str) -> bool {
        has_subagent_files(&self.projects_dir, self.fs_provider.as_ref(), project_id, session_id)
    }

    /// Find a subagent by ID.
    #[allow(dead_code)]
    pub fn find_subagent_by_id<'a>(subagents: &'a [Process], id: &str) -> Option<&'a Process> {
        subagents.iter().find(|s| s.id == id)
    }

    /// Get aggregated metrics across all subagents.
    #[allow(dead_code)]
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
#[cfg(test)]
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
    use crate::discovery::subagent_linking::{enrich_subagent_from_task, extract_team_message_summary};
    use crate::types::chunks::TeamInfo;
    use crate::types::domain::MessageType;
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
        let files = list_subagent_files(&resolver.projects_dir, resolver.fs_provider.as_ref(), "-Users-test-project", "session-123");
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

        let files = list_subagent_files(&resolver.projects_dir, resolver.fs_provider.as_ref(), "-Users-test-project", "session-123");
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
        let messages = crate::parsing::jsonl_parser::parse_jsonl_content(content);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].uuid, "abc-123");
        assert_eq!(messages[0].parent_uuid, Some("parent-456".to_string()));
    }

    #[test]
    fn test_parse_jsonl_content_missing_uuid() {
        // parse_jsonl_line skips messages with empty uuid, so this returns empty
        let content = r#"{"type":"user","message":{"role":"user","content":"Hello"},"timestamp":"2024-01-01T00:00:00Z"}"#;
        let messages = crate::parsing::jsonl_parser::parse_jsonl_content(content);
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

        let files = list_subagent_files(&resolver.projects_dir, resolver.fs_provider.as_ref(), "-Users-test-project", "session-456");
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

        let files = list_subagent_files(&resolver.projects_dir, resolver.fs_provider.as_ref(), "-Users-test-project", "session-456");
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

        let files = list_subagent_files(&resolver.projects_dir, resolver.fs_provider.as_ref(), "-Users-test-project", "session-456");
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
