//! IPC Handlers for Search Operations.
//!
//! Handlers:
//! - search_sessions: Search sessions in a project
//! - search_all_projects: Search sessions across all projects

use crate::discovery::SessionSearcher;
use crate::infrastructure::fs_provider::FsProvider;
use crate::types::domain::SearchSessionsResult;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::State;

/// Search sessions in a project.
#[tauri::command]
pub async fn search_sessions(
    project_id: String,
    query: String,
    max_results: Option<u32>,
    searcher: State<'_, Mutex<SessionSearcher>>,
) -> Result<SearchSessionsResult, String> {
    let max = max_results.unwrap_or(50).min(100).max(1);

    if query.trim().is_empty() {
        return Ok(SearchSessionsResult {
            results: Vec::new(),
            total_matches: 0,
            sessions_searched: 0,
            query,
            is_partial: None,
        });
    }

    let mut searcher = searcher.lock().map_err(|e| e.to_string())?;
    Ok(searcher.search_sessions(&project_id, &query, max))
}

/// Search sessions across all projects.
#[tauri::command]
pub async fn search_all_projects(
    query: String,
    max_results: Option<u32>,
    _searcher: State<'_, Mutex<SessionSearcher>>,
) -> Result<SearchSessionsResult, String> {
    let max = max_results.unwrap_or(50).min(100).max(1);

    if query.trim().is_empty() {
        return Ok(SearchSessionsResult {
            results: Vec::new(),
            total_matches: 0,
            sessions_searched: 0,
            query,
            is_partial: None,
        });
    }

    // TODO: Implement cross-project search
    // For now, return empty results
    Ok(SearchSessionsResult {
        results: Vec::new(),
        total_matches: 0,
        sessions_searched: 0,
        query,
        is_partial: None,
    })
}

/// Create a SessionSearcher state.
pub fn create_searcher_state(projects_dir: PathBuf, fs_provider: Arc<dyn FsProvider>) -> Mutex<SessionSearcher> {
    Mutex::new(SessionSearcher::new(projects_dir, fs_provider))
}