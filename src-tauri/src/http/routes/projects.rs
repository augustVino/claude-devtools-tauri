//! Projects 路由处理器 — 薄封装层。

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};

use crate::commands::guards;
use crate::http::state::HttpState;
use crate::types::domain::{Project, RepositoryGroup, Session};

use super::error_json;

/// GET /api/projects
pub async fn get_projects(
    State(state): State<HttpState>,
) -> Result<Json<Vec<Project>>, (StatusCode, Json<super::ErrorResponse>)> {
    let projects = state
        .project_service
        .scan_projects()
        .await
        .map_err(|e| error_json(e.to_string()))?;
    Ok(Json(projects))
}

/// GET /api/repository-groups
pub async fn get_repository_groups(
    State(state): State<HttpState>,
) -> Result<Json<Vec<RepositoryGroup>>, (StatusCode, Json<super::ErrorResponse>)> {
    let groups = state
        .project_service
        .get_repository_groups()
        .await
        .map_err(|e| error_json(e.to_string()))?;
    Ok(Json(groups))
}

/// GET /api/worktrees/{id}/sessions
pub async fn get_worktree_sessions(
    State(state): State<HttpState>,
    Path(worktree_id): Path<String>,
) -> Result<Json<Vec<Session>>, (StatusCode, Json<super::ErrorResponse>)> {
    let safe_id = guards::validate_project_id(&worktree_id).map_err(error_json)?;
    let sessions = state
        .project_service
        .get_worktree_sessions(&safe_id)
        .await
        .map_err(|e| error_json(e.to_string()))?;
    Ok(Json(sessions))
}
