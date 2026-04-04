//! Claude Root Path 变更时重建本地 ServiceContext。
//!
//! 当用户更改 claude root path 时，需要重建整个 local ServiceContext
//! （包括 FileWatcher、ProjectScanner、SessionSearcher、SubagentResolver），
//! 与 Electron 的 `reconfigureLocalContextForClaudeRoot()` 对齐。

use std::sync::Arc;

use super::service_context::{ContextType, ServiceContext, ServiceContextConfig};
use super::{ConfigManager, ContextManager, DataCache, FsProvider, LocalFsProvider, NotificationManager};
use crate::services::SearchService;

/// Rebuild the local ServiceContext when claude root path changes.
///
/// Creates a new ServiceContext with updated paths, replaces the old one in
/// ContextManager, spawns watcher tasks, and updates the SearchService.
pub async fn rebuild_local_context(
    context_manager: &Arc<tokio::sync::RwLock<ContextManager>>,
    notification_manager: &Arc<tokio::sync::RwLock<NotificationManager>>,
    config_manager: &Arc<ConfigManager>,
    cache: DataCache,
    app_handle: &tauri::AppHandle,
    search_service: &Arc<SearchService>,
) -> Result<(), String> {
    let projects_dir = crate::utils::get_projects_base_path();
    let todos_dir = crate::utils::get_todos_base_path();
    let fs_provider: Arc<dyn FsProvider> = Arc::new(LocalFsProvider::new());

    // Create new ServiceContext with updated paths
    let new_context = ServiceContext::new(ServiceContextConfig {
        id: "local".to_string(),
        context_type: ContextType::Local,
        projects_dir: projects_dir.clone(),
        todos_dir: todos_dir.clone(),
        fs_provider: fs_provider.clone(),
        cache: Some(cache),
    });

    // Replace old context (cancels old watcher tasks)
    {
        let mut cm = context_manager.write().await;
        cm.replace_context("local", new_context).await
            .map_err(|e| format!("Failed to replace local context: {e}"))?;
    }

    // Spawn watcher tasks for new context
    {
        let cm = context_manager.read().await;
        if let Some(local_ctx) = cm.get("local") {
            let local = local_ctx.read().await;
            local.spawn_watcher_tasks(
                app_handle.clone(),
                config_manager.clone(),
                notification_manager.clone(),
            ).await;
        }
    }

    // Update SearchService internal searcher with new paths
    search_service.rebuild(projects_dir, todos_dir, fs_provider)?;

    Ok(())
}
