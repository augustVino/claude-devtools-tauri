//! Projects 路由处理器。
//!
//! 对应 Tauri 命令：sessions::get_projects, projects::get_repository_groups, projects::get_worktree_sessions。

use axum::{Json, extract::State, http::StatusCode};

use crate::discovery::{ProjectScanner, WorktreeGrouper};
use crate::http::state::HttpState;
use crate::infrastructure::fs_provider::LocalFsProvider;
use crate::types::domain::{Project, RepositoryGroup, Session};
use crate::utils::{get_projects_base_path, get_todos_base_path};

use super::error_json;

/// 获取所有项目列表。
///
/// GET /api/projects
pub async fn get_projects(
    State(_state): State<HttpState>,
) -> Result<Json<Vec<Project>>, (StatusCode, Json<super::ErrorResponse>)> {
    let projects_dir = crate::utils::path_decoder::get_projects_base_path();
    let todos_dir = crate::utils::path_decoder::get_todos_base_path();
    let scanner = ProjectScanner::with_paths(
        projects_dir,
        todos_dir,
        std::sync::Arc::new(crate::infrastructure::fs_provider::LocalFsProvider::new()),
    );
    Ok(Json(scanner.scan()))
}

/// 获取按 git 仓库分组的项目列表。
///
/// GET /api/repository-groups
pub async fn get_repository_groups(
    State(_state): State<HttpState>,
) -> Result<Json<Vec<RepositoryGroup>>, (StatusCode, Json<super::ErrorResponse>)> {
    let projects_dir = get_projects_base_path();

    if !projects_dir.exists() {
        return Ok(Json(Vec::new()));
    }

    let scanner = ProjectScanner::with_paths(
        get_projects_base_path(),
        get_todos_base_path(),
        std::sync::Arc::new(LocalFsProvider::new()),
    );
    let projects = scanner.scan();

    if projects.is_empty() {
        return Ok(Json(Vec::new()));
    }

    let grouper = WorktreeGrouper::new(projects_dir);
    Ok(Json(grouper.group_by_repository(projects)))
}

/// 获取指定 worktree（项目）下的会话列表。
///
/// GET /api/worktrees/{id}/sessions
pub async fn get_worktree_sessions(
    State(_state): State<HttpState>,
    axum::extract::Path(worktree_id): axum::extract::Path<String>,
) -> Result<Json<Vec<Session>>, (StatusCode, Json<super::ErrorResponse>)> {
    let scanner = ProjectScanner::with_paths(
        get_projects_base_path(),
        get_todos_base_path(),
        std::sync::Arc::new(LocalFsProvider::new()),
    );
    Ok(Json(scanner.list_sessions(&worktree_id)))
}

