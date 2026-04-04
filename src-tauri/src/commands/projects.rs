//! IPC Handlers for Project Operations.

use tauri::{command, State};
use std::sync::Arc;

use crate::services::ProjectService;
use crate::types::domain::{RepositoryGroup, Session};

#[command]
pub async fn get_repository_groups(
    service: State<'_, Arc<ProjectService>>,
) -> Result<Vec<RepositoryGroup>, String> {
    Ok(service.get_repository_groups())
}

#[command]
pub async fn get_worktree_sessions(
    service: State<'_, Arc<ProjectService>>,
    worktree_id: String,
) -> Result<Vec<Session>, String> {
    Ok(service.get_worktree_sessions(&worktree_id))
}
