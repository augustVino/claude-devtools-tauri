//! IPC Handlers for Project Operations.

use std::sync::Arc;
use tauri::{command, State};

use crate::services::ProjectService;
use crate::types::domain::{RepositoryGroup, Session};

#[command]
pub async fn get_repository_groups(
    service: State<'_, Arc<dyn ProjectService>>,
) -> Result<Vec<RepositoryGroup>, String> {
    service
        .get_repository_groups()
        .await
        .map_err(|e| e.into_tauri_string())
}

#[command]
pub async fn get_worktree_sessions(
    service: State<'_, Arc<dyn ProjectService>>,
    worktree_id: String,
) -> Result<Vec<Session>, String> {
    service
        .get_worktree_sessions(&worktree_id)
        .await
        .map_err(|e| e.into_tauri_string())
}
