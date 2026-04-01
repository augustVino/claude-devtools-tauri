//! IPC Handlers for Project Operations.
//!
//! Handlers:
//! - get_repository_groups: List projects grouped by git repository
//! - get_worktree_sessions: List sessions for a specific worktree

use tauri::command;

use std::sync::Arc;

use crate::discovery::{ProjectScanner, WorktreeGrouper};
use crate::infrastructure::fs_provider::LocalFsProvider;
use crate::types::domain::{RepositoryGroup, Session};
use crate::utils::{get_projects_base_path, get_todos_base_path};

/// Get repository groups with worktree information.
#[command]
pub async fn get_repository_groups() -> Vec<RepositoryGroup> {
    let projects_dir = get_projects_base_path();

    if !projects_dir.exists() {
        return Vec::new();
    }

    let scanner = ProjectScanner::with_paths(
        get_projects_base_path(),
        get_todos_base_path(),
        Arc::new(LocalFsProvider::new()),
    );
    let projects = scanner.scan();

    if projects.is_empty() {
        return Vec::new();
    }

    let grouper = WorktreeGrouper::new(projects_dir);
    grouper.group_by_repository(projects)
}

/// Get sessions for a specific worktree (project).
#[command]
pub async fn get_worktree_sessions(worktree_id: String) -> Vec<Session> {
    let scanner = ProjectScanner::with_paths(
        get_projects_base_path(),
        get_todos_base_path(),
        Arc::new(LocalFsProvider::new()),
    );
    scanner.list_sessions(&worktree_id)
}
