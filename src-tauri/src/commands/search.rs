//! IPC Handlers for Search Operations — 薄封装层。

use tauri::{command, State};
use std::sync::Arc;

use crate::services::SearchService;
use crate::types::domain::{FindSessionByIdResult, FindSessionsByPartialIdResult, SearchSessionsResult};

#[tauri::command]
pub async fn search_sessions(
    service: State<'_, Arc<SearchService>>,
    project_id: String,
    query: String,
    max_results: Option<u32>,
) -> Result<SearchSessionsResult, String> {
    let max = max_results.unwrap_or(50).min(200).max(1);
    service.search_sessions(&project_id, &query, max).await
}

#[tauri::command]
pub async fn search_all_projects(
    service: State<'_, Arc<SearchService>>,
    query: String,
    max_results: Option<u32>,
) -> Result<SearchSessionsResult, String> {
    let max = max_results.unwrap_or(50).min(200).max(1);
    service.search_all_projects(&query, max).await
}

#[tauri::command]
pub async fn find_session_by_id(
    service: State<'_, Arc<SearchService>>,
    session_id: String,
) -> Result<FindSessionByIdResult, String> {
    let safe_id = crate::commands::guards::validate_session_id(&session_id)?;
    service.find_session_by_id(&safe_id).await
}

#[tauri::command]
pub async fn find_sessions_by_partial_id(
    service: State<'_, Arc<SearchService>>,
    fragment: String,
    max_results: Option<usize>,
) -> Result<FindSessionsByPartialIdResult, String> {
    let max = max_results.unwrap_or(20).min(100).max(1);
    service.find_sessions_by_partial_id(&fragment, max).await
}

