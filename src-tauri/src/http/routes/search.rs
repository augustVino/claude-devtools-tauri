//! Search 路由处理器。
//!
//! 对应 Tauri 命令：search.rs 中的搜索命令。

use axum::{Json, extract::State, http::StatusCode};

use crate::commands::guards;
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
    let safe_project_id = guards::validate_project_id(&project_id)
        .map_err(|e| error_json(e))?;

    let raw_query = params.get("q").cloned().unwrap_or_default();
    let max_results = params
        .get("maxResults")
        .and_then(|v| v.parse::<u32>().ok());

    let max = guards::coerce_search_max_results(max_results);

    if raw_query.trim().is_empty() {
        return Ok(Json(SearchSessionsResult {
            results: Vec::new(),
            total_matches: 0,
            sessions_searched: 0,
            query: raw_query,
            is_partial: None,
        }));
    }

    let query = guards::validate_search_query(&raw_query)
        .map_err(|e| error_json(e))?;

    // TODO: Wrap in tokio::task::spawn_blocking to avoid blocking the async runtime
    // (same issue as Tauri IPC commands — see commands/search.rs for the pattern).
    let mut searcher = state
        .searcher
        .lock()
        .map_err(|e| error_json(e.to_string()))?;
    Ok(Json(searcher.search_sessions(&safe_project_id, &query, max)))
}

/// 搜索所有项目中的会话。
///
/// GET /api/search?q=&maxResults=
pub async fn search_all_projects(
    State(state): State<HttpState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<SearchSessionsResult>, (StatusCode, Json<super::ErrorResponse>)> {
    let raw_query = params.get("q").cloned().unwrap_or_default();
    let max_results = params
        .get("maxResults")
        .and_then(|v| v.parse::<u32>().ok());

    let max = guards::coerce_search_max_results(max_results);

    if raw_query.trim().is_empty() {
        return Ok(Json(SearchSessionsResult {
            results: Vec::new(),
            total_matches: 0,
            sessions_searched: 0,
            query: raw_query,
            is_partial: None,
        }));
    }

    let query = guards::validate_search_query(&raw_query)
        .map_err(|e| error_json(e))?;

    let mut searcher = state
        .searcher
        .lock()
        .map_err(|e| error_json(e.to_string()))?;
    let result = searcher.search_all_projects(&query, max);
    Ok(Json(result))
}

/// Find a session by its exact UUID across all projects.
///
/// GET /api/sessions/{session_id}/locate
pub async fn find_session_by_id(
    State(state): State<HttpState>,
    axum::extract::Path(session_id): axum::extract::Path<String>,
) -> Result<Json<crate::types::domain::FindSessionByIdResult>, (StatusCode, Json<super::ErrorResponse>)> {
    let safe_session_id = guards::validate_session_id(&session_id)
        .map_err(|e| error_json(e))?;

    let mut searcher = state
        .searcher
        .lock()
        .map_err(|e| error_json(e.to_string()))?;
    Ok(Json(searcher.find_session_by_id(&safe_session_id)))
}

/// Find sessions whose IDs contain the given fragment (case-insensitive).
///
/// GET /api/sessions/search-by-id/{fragment}
pub async fn find_sessions_by_partial_id(
    State(state): State<HttpState>,
    axum::extract::Path(fragment): axum::extract::Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<crate::types::domain::FindSessionsByPartialIdResult>, (StatusCode, Json<super::ErrorResponse>)> {
    let max_results = params
        .get("maxResults")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(20)
        .min(100)
        .max(1);

    let mut searcher = state
        .searcher
        .lock()
        .map_err(|e| error_json(e.to_string()))?;
    Ok(Json(searcher.find_sessions_by_partial_id(&fragment, max_results)))
}
