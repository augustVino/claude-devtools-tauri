//! SSH commands — Tauri IPC handlers for SSH connection lifecycle.
//!
//! `ssh_connect` and `ssh_disconnect` replicate the full context switch
//! lifecycle: stop old watcher -> switch -> start new watcher -> emit events.
//! SSH contexts skip file watcher spawning (handled in ServiceContext).

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use tauri::{command, AppHandle, Manager, State};
use tokio::sync::RwLock;

use crate::events;
use crate::infrastructure::context_manager::ContextInfo;
use crate::infrastructure::service_context::{ContextType, ServiceContext, ServiceContextConfig};
use crate::infrastructure::{ContextManager, SshConnectionManager};
use crate::types::ssh::{
    SshConfigHostEntry, SshConnectionConfig, SshConnectionStatus, SshLastConnection, SshTestResult,
};
use super::sessions::AppState;

/// Construct dynamic SSH context ID from host.
fn ssh_context_id(host: &str) -> String {
    if host.is_empty() {
        "ssh".to_string()
    } else {
        format!("ssh-{}", host)
    }
}

/// Check if a context ID belongs to an SSH context.
fn is_ssh_context_id(id: &str) -> bool {
    id == "ssh" || id.starts_with("ssh-")
}

/// Connect to an SSH server and switch to the SSH context.
///
/// Full lifecycle:
/// 1. Establish SSH connection via SshConnectionManager
/// 2. Create SSH ServiceContext with SshFsProvider
/// 3. Register (or replace) SSH context in ContextManager
/// 4. Stop old context's watcher -> switch -> start new watcher -> emit context:changed
#[command]
pub async fn ssh_connect(
    app: AppHandle,
    ssh_manager: State<'_, Arc<RwLock<SshConnectionManager>>>,
    context_manager: State<'_, Arc<RwLock<ContextManager>>>,
    config: SshConnectionConfig,
) -> Result<SshConnectionStatus, String> {
    // 1. Establish SSH connection
    let username = config.username.clone();
    let status = ssh_manager.write().await.connect(config).await?;

    // If connection failed, return error status without switching context
    if matches!(status.state, crate::types::ssh::SshConnectionState::Error) {
        return Ok(status);
    }

    // 2. Build SSH ServiceContext
    let host = status.host.clone().unwrap_or_default();
    let remote_projects_path = status
        .remote_projects_path
        .clone()
        .unwrap_or_else(|| format!("/home/{}/.claude/projects", username));
    let remote_todos_path = PathBuf::from(&remote_projects_path)
        .parent()
        .map(|p| p.join("todos"))
        .unwrap_or_else(|| PathBuf::from("/tmp/claude-todos-ssh"));

    let fs_provider: Arc<dyn crate::infrastructure::FsProvider> = {
        let mgr = ssh_manager.read().await;
        mgr.get_provider().await
            .ok_or_else(|| "SSH provider not available after connect".to_string())?
    };

    let shared_cache = {
        let state: tauri::State<Arc<tokio::sync::RwLock<AppState>>> = app.state();
        state.inner().read().await.cache.clone()
    };
    let ssh_context = ServiceContext::new(ServiceContextConfig {
        id: ssh_context_id(&host),
        context_type: ContextType::Ssh,
        projects_dir: PathBuf::from(&remote_projects_path),
        todos_dir: remote_todos_path,
        fs_provider,
        cache: Some(shared_cache),
    });

    // 3. If already on SSH context, tear down and switch back to local first
    // (mirrors Electron: destroy existing SSH context before creating new one)
    {
        let mut mgr = context_manager.write().await;
        if is_ssh_context_id(mgr.get_active_id()) {
            log::info!("SSH connect: already on SSH context, tearing down before reconnect");
            // Stop SSH watcher tasks
            let old_ssh_id = mgr.get_active_id().to_string();
            if let Some(ssh_ctx) = mgr.get(&old_ssh_id) {
                ssh_ctx.read().await.stop_watcher_tasks().await;
            }
            // Switch back to local
            if let Ok(result) = mgr.switch("local") {
                // Start local watcher tasks
                if let Some(local_ctx) = mgr.get(&result.current_id) {
                    let local = local_ctx.read().await;
                    let config_manager = app
                        .state::<Arc<crate::infrastructure::ConfigManager>>()
                        .inner()
                        .clone();
                    let notification_manager = app
                        .state::<Arc<RwLock<crate::infrastructure::NotificationManager>>>()
                        .inner()
                        .clone();
                    local.spawn_watcher_tasks(app.clone(), config_manager, notification_manager).await;
                }
            }
            // Destroy old SSH context
            let _ = mgr.destroy_context(&old_ssh_id).await;
        }
    }

    // 4. Register SSH context and switch
    {
        let mut mgr = context_manager.write().await;

        mgr.register_context(ssh_context)?;

        // Perform context switch
        let result = mgr.switch(&ssh_context_id(&host))?;
        log::info!(
            "SSH connect: context switched {} -> {}",
            result.previous_id,
            result.current_id
        );

        // Stop old context's watcher tasks
        if let Some(old_ctx) = mgr.get(&result.previous_id) {
            old_ctx.read().await.stop_watcher_tasks().await;
        }

        // Start new context's watcher tasks (SSH context skips watchers internally)
        if let Some(new_ctx) = mgr.get(&result.current_id) {
            let new = new_ctx.read().await;
            let config_manager = app
                .state::<Arc<crate::infrastructure::ConfigManager>>()
                .inner()
                .clone();
            let notification_manager = app
                .state::<Arc<RwLock<crate::infrastructure::NotificationManager>>>()
                .inner()
                .clone();
            new.spawn_watcher_tasks(app.clone(), config_manager, notification_manager).await;
        }

        // Emit context:changed event
        let ctx_arc = mgr
            .get(&result.current_id)
            .ok_or("SSH context not found after switch")?;
        let info = ContextInfo::from_context(&*ctx_arc.read().await);
        drop(mgr);

        events::emit_context_changed(&app, &info);

        // Bridge to SSE
        if let Some(broadcaster) = app.try_state::<crate::http::sse::SSEBroadcaster>() {
            broadcaster
                .inner()
                .send(crate::http::sse::BackendEvent::ContextChanged(info));
        }
    }

    Ok(status)
}

/// Disconnect from the SSH server and switch back to local context.
///
/// Full lifecycle:
/// 1. Stop SSH watcher tasks
/// 2. Switch to local context
/// 3. Start local watcher tasks
/// 4. Emit context:changed
/// 5. Destroy SSH context
/// 6. Disconnect SSH connection
#[command]
pub async fn ssh_disconnect(
    app: AppHandle,
    ssh_manager: State<'_, Arc<RwLock<SshConnectionManager>>>,
    context_manager: State<'_, Arc<RwLock<ContextManager>>>,
) -> Result<SshConnectionStatus, String> {
    // Check if SSH context is currently active
    {
        let mgr = context_manager.read().await;
        if !is_ssh_context_id(mgr.get_active_id()) {
            // Not on SSH context — just disconnect the SSH connection if any
            let status = ssh_manager.write().await.disconnect().await?;
            return Ok(status);
        }
    }

    // Perform context switch lifecycle
    {
        let mut mgr = context_manager.write().await;

        let result = mgr.switch("local")?;
        log::info!(
            "SSH disconnect: context switched {} -> {}",
            result.previous_id,
            result.current_id
        );

        // Stop old context's (SSH) watcher tasks
        if let Some(old_ctx) = mgr.get(&result.previous_id) {
            old_ctx.read().await.stop_watcher_tasks().await;
        }

        // Start new context's (local) watcher tasks
        if let Some(new_ctx) = mgr.get(&result.current_id) {
            let new = new_ctx.read().await;
            let config_manager = app
                .state::<Arc<crate::infrastructure::ConfigManager>>()
                .inner()
                .clone();
            let notification_manager = app
                .state::<Arc<RwLock<crate::infrastructure::NotificationManager>>>()
                .inner()
                .clone();
            new.spawn_watcher_tasks(app.clone(), config_manager, notification_manager).await;
        }

        // Emit context:changed event
        let ctx_arc = mgr
            .get(&result.current_id)
            .ok_or("Local context not found after switch")?;
        let info = ContextInfo::from_context(&*ctx_arc.read().await);

        // Destroy SSH context
        mgr.destroy_context(&result.previous_id).await?;
        drop(mgr);

        events::emit_context_changed(&app, &info);

        // Bridge to SSE
        if let Some(broadcaster) = app.try_state::<crate::http::sse::SSEBroadcaster>() {
            broadcaster
                .inner()
                .send(crate::http::sse::BackendEvent::ContextChanged(info));
        }
    }

    // Disconnect SSH connection
    let status = ssh_manager.write().await.disconnect().await?;
    Ok(status)
}

/// Get the current SSH connection state.
#[command]
pub async fn ssh_get_state(
    ssh_manager: State<'_, Arc<RwLock<SshConnectionManager>>>,
) -> Result<SshConnectionStatus, String> {
    Ok(ssh_manager.read().await.get_active_state().await)
}

/// Test an SSH connection configuration without actually connecting.
#[command]
pub async fn ssh_test(
    ssh_manager: State<'_, Arc<RwLock<SshConnectionManager>>>,
    config: SshConnectionConfig,
) -> Result<SshTestResult, String> {
    ssh_manager.read().await.test(&config).await
}

/// Get all host entries from the SSH config file.
#[command]
pub async fn ssh_get_config_hosts(
    ssh_manager: State<'_, Arc<RwLock<SshConnectionManager>>>,
) -> Result<Vec<SshConfigHostEntry>, String> {
    Ok(ssh_manager.read().await.get_config_hosts())
}

/// Resolve a host alias from the SSH config.
#[command]
pub async fn ssh_resolve_host(
    ssh_manager: State<'_, Arc<RwLock<SshConnectionManager>>>,
    alias: String,
) -> Result<Option<SshConfigHostEntry>, String> {
    Ok(ssh_manager.read().await.resolve_host_config(&alias))
}

/// Save the last SSH connection configuration via ConfigManager.
#[command]
pub async fn ssh_save_last_connection(
    connection: SshLastConnection,
    config_manager: State<'_, Arc<crate::infrastructure::ConfigManager>>,
) -> Result<(), String> {
    let connection_value = serde_json::json!({
        "lastConnection": {
            "host": connection.host,
            "port": connection.port,
            "username": connection.username,
            "authMethod": connection.auth_method,
            "privateKeyPath": connection.private_key_path,
        }
    });
    config_manager.update_config("ssh", connection_value).await?;
    Ok(())
}

/// Get the last SSH connection configuration from ConfigManager.
#[command]
pub async fn ssh_get_last_connection(
    config_manager: State<'_, Arc<crate::infrastructure::ConfigManager>>,
) -> Result<Option<SshLastConnection>, String> {
    let config = config_manager.get_config().await;
    let last = config.ssh.as_ref().and_then(|s| s.last_connection.as_ref());
    Ok(last.map(|c| SshLastConnection {
        host: c.host.clone(),
        port: c.port,
        username: c.username.clone(),
        auth_method: c.auth_method.clone(),
        private_key_path: c.private_key_path.clone(),
    }))
}
