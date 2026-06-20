//! SSH commands — Tauri IPC handlers for SSH connection lifecycle.
//!
//! All business logic delegated to SshService. These handlers are thin adapters:
//! State injection → error wrapping → SSE broadcaster bridging.

use std::sync::Arc;
use tauri::{command, AppHandle, Manager, State};

use crate::services::SshService;
use crate::types::ssh::{
    SshConfigHostEntry, SshConnectionConfig, SshConnectionStatus, SshLastConnection, SshTestResult,
};

#[command]
pub async fn ssh_connect(
    app: AppHandle,
    ssh_svc: State<'_, Arc<dyn SshService>>,
    config: SshConnectionConfig,
) -> Result<SshConnectionStatus, String> {
    let broadcaster = app
        .try_state::<crate::http::sse::SSEBroadcaster>()
        .map(|s| s.inner().clone());
    // AppHandle 不再传入 service（H3: 已在构造时存储于 SshServiceImpl 内部）
    ssh_svc
        .connect(config, broadcaster.as_ref())
        .await
        .map_err(|e| e.into_tauri_string())
}

#[command]
pub async fn ssh_disconnect(
    app: AppHandle,
    ssh_svc: State<'_, Arc<dyn SshService>>,
) -> Result<SshConnectionStatus, String> {
    let broadcaster = app
        .try_state::<crate::http::sse::SSEBroadcaster>()
        .map(|s| s.inner().clone());
    ssh_svc
        .disconnect(broadcaster.as_ref())
        .await
        .map_err(|e| e.into_tauri_string())
}

#[command]
pub async fn ssh_get_state(
    ssh_svc: State<'_, Arc<dyn SshService>>,
) -> Result<SshConnectionStatus, String> {
    Ok(ssh_svc.get_active_state().await)
}

#[command]
pub async fn ssh_test(
    ssh_svc: State<'_, Arc<dyn SshService>>,
    config: SshConnectionConfig,
) -> Result<SshTestResult, String> {
    ssh_svc
        .test(&config)
        .await
        .map_err(|e| e.into_tauri_string())
}

#[command]
pub async fn ssh_get_config_hosts(
    ssh_svc: State<'_, Arc<dyn SshService>>,
) -> Result<Vec<SshConfigHostEntry>, String> {
    Ok(ssh_svc.get_config_hosts().await)
}

#[command]
pub async fn ssh_resolve_host(
    ssh_svc: State<'_, Arc<dyn SshService>>,
    alias: String,
) -> Result<Option<SshConfigHostEntry>, String> {
    Ok(ssh_svc.resolve_host_config(&alias).await)
}

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
    config_manager
        .update_config("ssh", connection_value)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

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
