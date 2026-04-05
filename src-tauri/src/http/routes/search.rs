//! Search 路由处理器 — 薄封装层。

use axum::{Json, extract::{Path, State}, http::StatusCode};

use crate::commands::guards;
use crate::http::state::HttpState;
use crate::types::domain::{FindSessionByIdResult, FindSessionsByPartialIdResult, SearchSessionsResult};

use super::error_json;

/// GET /api/projects/{project_id}/search?q=&maxResults=
pub async fn search_sessions(
    State(state): State<HttpState>,
    Path(project_id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<SearchSessionsResult>, (StatusCode, Json<super::ErrorResponse>)> {
    let safe_id = guards::validate_project_id(&project_id).map_err(error_json)?;
    let raw_query = params.get("q").cloned().unwrap_or_default();
    let max_results = params.get("maxResults").and_then(|v| v.parse::<u32>().ok());
    let max = guards::coerce_search_max_results(max_results);

    if raw_query.trim().is_empty() {
        return Ok(Json(SearchSessionsResult {
            results: Vec::new(), total_matches: 0, sessions_searched: 0,
            query: raw_query, is_partial: None,
        }));
    }

    let query = guards::validate_search_query(&raw_query).map_err(error_json)?;

    state.search_service.search_sessions(&safe_id, &query, max).await
        .map(Json)
        .map_err(|e| error_json(e.to_string()))
}

/// GET /api/search?q=&maxResults=
pub async fn search_all_projects(
    State(state): State<HttpState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<SearchSessionsResult>, (StatusCode, Json<super::ErrorResponse>)> {
    let raw_query = params.get("q").cloned().unwrap_or_default();
    let max_results = params.get("maxResults").and_then(|v| v.parse::<u32>().ok());
    let max = guards::coerce_search_max_results(max_results);

    if raw_query.trim().is_empty() {
        return Ok(Json(SearchSessionsResult {
            results: Vec::new(), total_matches: 0, sessions_searched: 0,
            query: raw_query, is_partial: None,
        }));
    }

    let query = guards::validate_search_query(&raw_query).map_err(error_json)?;

    state.search_service.search_all_projects(&query, max).await
        .map(Json)
        .map_err(|e| error_json(e.to_string()))
}

/// GET /api/sessions/{session_id}/locate
pub async fn find_session_by_id(
    State(state): State<HttpState>,
    Path(session_id): Path<String>,
) -> Result<Json<FindSessionByIdResult>, (StatusCode, Json<super::ErrorResponse>)> {
    let safe_id = guards::validate_session_id(&session_id).map_err(error_json)?;
    state.search_service.find_session_by_id(&safe_id).await
        .map(Json)
        .map_err(|e| error_json(e.to_string()))
}

/// GET /api/sessions/search-by-id/{fragment}
pub async fn find_sessions_by_partial_id(
    State(state): State<HttpState>,
    Path(fragment): Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<FindSessionsByPartialIdResult>, (StatusCode, Json<super::ErrorResponse>)> {
    let frag = fragment.trim().to_string();
    if frag.len() < 3 {
        return Ok(Json(FindSessionsByPartialIdResult { found: false, results: vec![] }));
    }

    let max = params.get("maxResults")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(20).min(100).max(1);

    state.search_service.find_sessions_by_partial_id(&frag, max).await
        .map(Json)
        .map_err(|e| error_json(e.to_string()))
}
