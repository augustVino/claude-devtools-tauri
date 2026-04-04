//! Sessions 路由处理器 — 薄封装层。
//!
//! 所有业务逻辑委托给 [`SessionService`](crate::services::SessionService)。
//! 本模块仅负责 Axum 参数提取、校验和格式转换。

use axum::{Json, extract::{Path, State}, http::StatusCode};

use crate::commands::guards;
use crate::http::state::HttpState;
use crate::services::SessionService;
use crate::types::domain::{PaginatedSessionsResult, Session, SessionMetrics};
use crate::types::chunks::{ConversationGroup, SessionDetail};

use super::error_json;

/// 路径参数：project_id + session_id。
#[derive(serde::Deserialize)]
pub struct ProjectSessionPath {
    pub project_id: String,
    pub session_id: String,
}

// =============================================================================
// 会话列表路由
// =============================================================================

pub async fn get_sessions(
    State(state): State<HttpState>,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<Session>>, (StatusCode, Json<super::ErrorResponse>)> {
    let safe_id = guards::validate_project_id(&project_id).map_err(error_json)?;
    state.session_service.get_sessions(&safe_id).await
        .map(Json)
        .map_err(error_json)
}

pub async fn get_sessions_paginated(
    State(state): State<HttpState>,
    Path(project_id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<PaginatedSessionsResult>, (StatusCode, Json<super::ErrorResponse>)> {
    let safe_id = guards::validate_project_id(&project_id).map_err(error_json)?;
    let cursor = params.get("cursor").map(|s| s.as_str());
    let limit = params.get("limit").and_then(|v| v.parse::<u32>().ok());
    let page_limit = guards::coerce_limit(limit, 50, 100) as u32;

    state.session_service.get_sessions_paginated(
        &safe_id, cursor, Some(page_limit), None,
    ).await
        .map(Json)
        .map_err(error_json)
}

/// 请求体：批量获取会话。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionsByIdsRequest {
    pub session_ids: Vec<String>,
    #[allow(dead_code)]
    pub metadata_level: Option<String>,
}

pub async fn get_sessions_by_ids(
    State(state): State<HttpState>,
    Path(project_id): Path<String>,
    Json(body): Json<SessionsByIdsRequest>,
) -> Result<Json<Vec<Session>>, (StatusCode, Json<super::ErrorResponse>)> {
    let safe_id = guards::validate_project_id(&project_id).map_err(error_json)?;

    const MAX: usize = 50;
    let ids: Vec<String> = body.session_ids.into_iter().take(MAX).collect();

    state.session_service.get_sessions_by_ids(&safe_id, &ids).await
        .map(Json)
        .map_err(error_json)
}

// =============================================================================
// 会话详情路由
// =============================================================================

pub async fn get_session_detail(
    State(state): State<HttpState>,
    Path(path): Path<ProjectSessionPath>,
) -> Result<Json<Option<SessionDetail>>, (StatusCode, Json<super::ErrorResponse>)> {
    let safe_pid = guards::validate_project_id(&path.project_id).map_err(error_json)?;
    let safe_sid = guards::validate_session_id(&path.session_id).map_err(error_json)?;

    state.session_service.get_session_detail(&safe_pid, &safe_sid).await
        .map(Json)
        .map_err(error_json)
}

pub async fn get_session_metrics(
    State(state): State<HttpState>,
    Path(path): Path<ProjectSessionPath>,
) -> Result<Json<Option<SessionMetrics>>, (StatusCode, Json<super::ErrorResponse>)> {
    let safe_pid = guards::validate_project_id(&path.project_id).map_err(error_json)?;
    let safe_sid = guards::validate_session_id(&path.session_id).map_err(error_json)?;

    state.session_service.get_session_metrics(&safe_pid, &safe_sid).await
        .map(Json)
        .map_err(error_json)
}

// =============================================================================
// 派生数据路由
// =============================================================================

pub async fn get_session_groups(
    State(state): State<HttpState>,
    Path(path): Path<ProjectSessionPath>,
) -> Result<Json<Vec<ConversationGroup>>, (StatusCode, Json<super::ErrorResponse>)> {
    let safe_pid = guards::validate_project_id(&path.project_id).map_err(error_json)?;
    let safe_sid = guards::validate_session_id(&path.session_id).map_err(error_json)?;

    state.session_service.get_session_groups(&safe_pid, &safe_sid).await
        .map(Json)
        .map_err(error_json)
}

pub async fn get_waterfall_data(
    State(state): State<HttpState>,
    Path(path): Path<ProjectSessionPath>,
) -> Result<
    Json<Option<crate::analysis::waterfall_builder::WaterfallData>>,
    (StatusCode, Json<super::ErrorResponse>),
> {
    let safe_pid = guards::validate_project_id(&path.project_id).map_err(error_json)?;
    let safe_sid = guards::validate_session_id(&path.session_id).map_err(error_json)?;

    state.session_service.get_waterfall_data(&safe_pid, &safe_sid).await
        .map(Json)
        .map_err(error_json)
}
