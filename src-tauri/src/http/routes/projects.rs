//! Projects 路由处理器。
//!
//! 对应 Tauri 命令：sessions::get_projects, projects::get_repository_groups, projects::get_worktree_sessions。

use axum::{Json, extract::State, http::StatusCode};

use crate::discovery::{ProjectScanner, WorktreeGrouper};
use crate::http::state::HttpState;
use crate::types::domain::{Project, RepositoryGroup, Session};
use crate::utils::{decode_path, extract_base_dir, extract_project_name, get_projects_base_path};

use super::error_json;

/// 获取所有项目列表。
///
/// GET /api/projects
pub async fn get_projects(
    State(_state): State<HttpState>,
) -> Result<Json<Vec<Project>>, (StatusCode, Json<super::ErrorResponse>)> {
    let base_path = get_projects_base_path();

    if !base_path.exists() {
        return Ok(Json(vec![]));
    }

    let mut projects = vec![];
    let mut entries = tokio::fs::read_dir(&base_path)
        .await
        .map_err(|e| error_json(e.to_string()))?;

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.is_dir() {
            let project_id = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let project_path = decode_path(&project_id);
            let name = extract_project_name(&project_id, None);

            // 统计会话数量
            let session_count = count_sessions_in_dir(&path).await;

            projects.push(Project {
                id: project_id,
                path: project_path,
                name,
                sessions: vec![],
                created_at: path
                    .metadata()
                    .and_then(|m| m.created())
                    .map(|t| {
                        t.duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64
                    })
                    .unwrap_or(0),
                most_recent_session: None,
            });
        }
    }

    // 按创建时间降序排列
    projects.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    Ok(Json(projects))
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

    let scanner = ProjectScanner::new();
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
    let scanner = ProjectScanner::new();
    Ok(Json(scanner.list_sessions(&worktree_id)))
}

/// 统计目录下的 `.jsonl` 会话文件数量。
async fn count_sessions_in_dir(dir: &std::path::Path) -> u32 {
    let mut count = 0u32;
    if let Ok(mut entries) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry
                .path()
                .extension()
                .map(|e| e == "jsonl")
                .unwrap_or(false)
            {
                count += 1;
            }
        }
    }
    count
}
