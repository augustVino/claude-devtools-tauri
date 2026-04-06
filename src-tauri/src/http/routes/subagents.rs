//! Subagent HTTP routes — REST endpoints for sub-agent detail queries.
//!
//! Business logic delegated to SubagentService. Error handling uses AppError IntoResponse.

use axum::{
    Json,
    extract::{Path, State},
    routing::get,
};
use crate::commands::guards;
use crate::error::AppError;
use crate::http::state::HttpState;
use crate::types::chunks::SubagentDetail;

/// GET /api/projects/:projectId/sessions/:sessionId/subagents/:subagentId
pub async fn get_subagent_detail(
    State(state): State<HttpState>,
    Path((project_id, session_id, subagent_id)): Path<(String, String, String)>,
) -> Result<Json<Option<SubagentDetail>>, AppError> {
    // 【审查修正】补上输入校验（原计划遗漏）
    // 【审查修正 #6】保留服务端日志（降级 error → warn：客户端输入问题非系统故障）
    let safe_project_id = guards::validate_project_id(&project_id)
        .map_err(|e| { log::warn!("Invalid projectId: {e}"); AppError::InvalidInput(e) })?;
    let safe_session_id = guards::validate_session_id(&session_id)
        .map_err(|e| { log::warn!("Invalid sessionId: {e}"); AppError::InvalidInput(e) })?;
    let safe_subagent_id = guards::validate_subagent_id(&subagent_id)
        .map_err(|e| { log::warn!("Invalid subagentId: {e}"); AppError::InvalidInput(e) })?;

    let detail = state.subagent_svc.get_subagent_detail(&safe_project_id, &safe_session_id, &safe_subagent_id).await?;
    Ok(Json(detail))
}

/// Build subagent routes.
pub fn routes() -> axum::Router<HttpState> {
    axum::Router::new()
        .route("/api/projects/{projectId}/sessions/{sessionId}/subagents/{subagentId}", get(get_subagent_detail))
}
