//! Search 路由处理器。
//!
//! 对应 Tauri 命令：search.rs 中的搜索命令。

use axum::{Json, extract::State, http::StatusCode};

use crate::http::state::HttpState;
use crate::types::domain::SearchSessionsResult;

use super::error_json;

/// 搜索指定项目中的会话。
///
/// GET /api/projects/{project_id}/search?q=&maxResults=
pub async fn search_sessions(
    State(state): State<HttpState>,
    axum::extract::Path(project_id): axum::extract::Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<SearchSessionsResult>, (StatusCode, Json<super::ErrorResponse>)> {
    let query = params.get("q").cloned().unwrap_or_default();
    let max_results = params
        .get("maxResults")
        .and_then(|v| v.parse::<u32>().ok());

    let max = max_results.unwrap_or(50).min(200).max(1);

    if query.trim().is_empty() {
        return Ok(Json(SearchSessionsResult {
            results: Vec::new(),
            total_matches: 0,
            sessions_searched: 0,
            query,
            is_partial: None,
        }));
    }

    // TODO: Wrap in tokio::task::spawn_blocking to avoid blocking the async runtime
    // (same issue as Tauri IPC commands — see commands/search.rs for the pattern).
    let mut searcher = state
        .searcher
        .lock()
        .map_err(|e| error_json(e.to_string()))?;
    Ok(Json(searcher.search_sessions(&project_id, &query, max)))
}

/// 搜索所有项目中的会话。
///
/// GET /api/search?q=&maxResults=
pub async fn search_all_projects(
    State(_state): State<HttpState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<SearchSessionsResult>, (StatusCode, Json<super::ErrorResponse>)> {
    let query = params.get("q").cloned().unwrap_or_default();
    let max_results = params
        .get("maxResults")
        .and_then(|v| v.parse::<u32>().ok());

    let _max = max_results.unwrap_or(50).min(200).max(1);

    if query.trim().is_empty() {
        return Ok(Json(SearchSessionsResult {
            results: Vec::new(),
            total_matches: 0,
            sessions_searched: 0,
            query,
            is_partial: None,
        }));
    }

    // TODO: Implement cross-project search
    // For now, return empty results
    Ok(SearchSessionsResult {
        results: Vec::new(),
        total_matches: 0,
        sessions_searched: 0,
        query,
        is_partial: None,
    }
    .into())
}
