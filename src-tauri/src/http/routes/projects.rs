//! Projects 路由处理器 — 薄封装层。

use axum::{Json, extract::{Path, State}, http::StatusCode};

use crate::commands::guards;
use crate::http::state::HttpState;
use crate::services::ProjectService;
use crate::types::domain::{Project, RepositoryGroup, Session};

use super::error_json;

/// GET /api/projects
pub async fn get_projects(
    State(state): State<HttpState>,
) -> Result<Json<Vec<Project>>, (StatusCode, Json<super::ErrorResponse>)> {
    Ok(Json(state.project_service.scan_projects()))
}

/// GET /api/repository-groups
pub async fn get_repository_groups(
    State(state): State<HttpState>,
) -> Result<Json<Vec<RepositoryGroup>>, (StatusCode, Json<super::ErrorResponse>)> {
    Ok(Json(state.project_service.get_repository_groups()))
}

/// GET /api/worktrees/{id}/sessions
pub async fn get_worktree_sessions(
    State(state): State<HttpState>,
    Path(worktree_id): Path<String>,
) -> Result<Json<Vec<Session>>, (StatusCode, Json<super::ErrorResponse>)> {
    let safe_id = guards::validate_project_id(&worktree_id).map_err(error_json)?;
    Ok(Json(state.project_service.get_worktree_sessions(&safe_id)))
}
