//! IPC Handlers for Subagent Operations.
//!
//! Handlers:
//! - get_subagent_detail: Get detailed info about a subagent session

use serde::Serialize;
use tauri::command;

use crate::discovery::SubagentResolver;
use crate::parsing::parse_session_file;
use crate::types::domain::SessionMetrics;
use crate::utils::get_projects_base_path;

/// Subagent detail with full session data.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentDetail {
    pub id: String,
    pub file_path: String,
    pub start_time_ms: u64,
    pub end_time_ms: u64,
    pub duration_ms: u64,
    pub is_parallel: bool,
    pub is_ongoing: bool,
    pub metrics: SessionMetrics,
    pub messages: Vec<crate::types::messages::ParsedMessage>,
    pub task_id: Option<String>,
}

/// Get detailed information about a subagent.
#[command]
pub async fn get_subagent_detail(
    project_id: String,
    session_id: String,
    subagent_id: String,
) -> Option<SubagentDetail> {
    let projects_dir = get_projects_base_path();

    // Construct path to subagent file (files are named "agent-{id}.jsonl")
    let base_dir = crate::utils::path_decoder::extract_base_dir(&project_id);
    let subagent_path = projects_dir
        .join(base_dir)
        .join(&session_id)
        .join("subagents")
        .join(format!("agent-{}.jsonl", subagent_id));

    if !subagent_path.exists() {
        return None;
    }

    // Parse subagent file
    let parsed = parse_session_file(&subagent_path).await;

    // Resolve subagent to get timing/parallel info
    let resolver = SubagentResolver::new(projects_dir, std::sync::Arc::new(crate::infrastructure::fs_provider::LocalFsProvider::new()));
    let processes = resolver.resolve_subagents(&project_id, &session_id);

    // Find matching process
    let process = processes.iter().find(|p| p.id == subagent_id);

    let (start_time_ms, end_time_ms, duration_ms, is_parallel, is_ongoing, task_id) = process
        .map(|p| (
            p.start_time_ms,
            p.end_time_ms,
            p.duration_ms,
            p.is_parallel,
            p.is_ongoing,
            p.task_id.clone()
        ))
        .unwrap_or((0, 0, 0, false, false, None));

    Some(SubagentDetail {
        id: subagent_id,
        file_path: subagent_path.to_string_lossy().to_string(),
        start_time_ms,
        end_time_ms,
        duration_ms,
        is_parallel,
        is_ongoing,
        metrics: parsed.metrics,
        messages: parsed.messages,
        task_id,
    })
}