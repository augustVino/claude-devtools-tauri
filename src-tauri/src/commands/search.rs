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
///
/// Uses `spawn_blocking` to avoid holding `std::sync::Mutex` across `.await`
/// points on the tokio async runtime.
#[tauri::command]
pub async fn search_sessions(
    project_id: String,
    query: String,
    max_results: Option<u32>,
    searcher: State<'_, Arc<Mutex<SessionSearcher>>>,
) -> Result<SearchSessionsResult, String> {
    let max = max_results.unwrap_or(50).min(200).max(1);

    if query.trim().is_empty() {
        return Ok(SearchSessionsResult {
            results: Vec::new(),
            total_matches: 0,
            sessions_searched: 0,
            query,
            is_partial: None,
        });
    }

    let searcher = searcher.inner().clone();
    let result = tokio::task::spawn_blocking(move || -> Result<SearchSessionsResult, String> {
        let mut searcher = searcher.lock().map_err(|e| e.to_string())?;
        Ok(searcher.search_sessions(&project_id, &query, max))
    })
    .await
    .map_err(|e| format!("search task panicked: {}", e))?;
    result
}

/// Search sessions across all projects.
///
/// Uses `spawn_blocking` to avoid holding `std::sync::Mutex` across `.await`
/// points on the tokio async runtime.
#[tauri::command]
pub async fn search_all_projects(
    query: String,
    max_results: Option<u32>,
    searcher: State<'_, Arc<Mutex<SessionSearcher>>>,
) -> Result<SearchSessionsResult, String> {
    let max = max_results.unwrap_or(50).min(200).max(1);

    if query.trim().is_empty() {
        return Ok(SearchSessionsResult {
            results: Vec::new(),
            total_matches: 0,
            sessions_searched: 0,
            query,
            is_partial: None,
        });
    }

    let searcher = searcher.inner().clone();
    let result = tokio::task::spawn_blocking(move || -> Result<SearchSessionsResult, String> {
        let mut searcher = searcher.lock().map_err(|e| e.to_string())?;
        Ok(searcher.search_all_projects(&query, max))
    })
    .await
    .map_err(|e| format!("search task panicked: {}", e))?;
    result
}

/// Create a SessionSearcher state wrapped in `Arc<Mutex<...>>` so it can be
/// cloned into `spawn_blocking` closures without holding the lock.
pub fn create_searcher_state(
    projects_dir: PathBuf,
    todos_dir: PathBuf,
    fs_provider: Arc<dyn FsProvider>,
) -> Arc<Mutex<SessionSearcher>> {
    Arc::new(Mutex::new(SessionSearcher::new(projects_dir, todos_dir, fs_provider, None)))
}

/// Find a session by its exact UUID across all projects.
#[tauri::command]
pub async fn find_session_by_id(
    session_id: String,
    searcher: State<'_, Arc<Mutex<SessionSearcher>>>,
) -> Result<crate::types::domain::FindSessionByIdResult, String> {
    let safe_session_id = crate::commands::guards::validate_session_id(&session_id)?;

    let searcher = searcher.inner().clone();
    let result = tokio::task::spawn_blocking(move || -> Result<crate::types::domain::FindSessionByIdResult, String> {
        let mut searcher = searcher.lock().map_err(|e| e.to_string())?;
        Ok(searcher.find_session_by_id(&safe_session_id))
    })
    .await
    .map_err(|e| format!("find_session_by_id task panicked: {}", e))?;
    result
}

/// Find sessions whose IDs contain the given fragment (case-insensitive).
#[tauri::command]
pub async fn find_sessions_by_partial_id(
    fragment: String,
    max_results: Option<usize>,
    searcher: State<'_, Arc<Mutex<SessionSearcher>>>,
) -> Result<crate::types::domain::FindSessionsByPartialIdResult, String> {
    let max = max_results.unwrap_or(20).min(100).max(1);

    let searcher = searcher.inner().clone();
    let result = tokio::task::spawn_blocking(move || -> Result<crate::types::domain::FindSessionsByPartialIdResult, String> {
        let mut searcher = searcher.lock().map_err(|e| e.to_string())?;
        Ok(searcher.find_sessions_by_partial_id(&fragment, max))
    })
    .await
    .map_err(|e| format!("find_sessions_by_partial_id task panicked: {}", e))?;
    result
}
