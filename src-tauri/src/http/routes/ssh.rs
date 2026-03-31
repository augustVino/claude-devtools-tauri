//! SSH HTTP routes — REST endpoints for SSH connection lifecycle.
//!
//! Aligns response formats with `httpClient.ts` expectations:
//! - connect/disconnect/state: `Json<SshConnectionStatus>` direct
//! - test: `Json<SshTestResult>` direct
//! - config-hosts/resolve-host/last-connection: `{ success: true, data: T }` wrapped
//! - save-last-connection: `{ success: true }` wrapped

use std::path::PathBuf;
use std::sync::Arc;
use tauri::Manager;

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use serde::Deserialize;

use crate::http::sse::BackendEvent;
use crate::http::state::HttpState;
use crate::infrastructure::context_manager::ContextInfo;
use crate::infrastructure::service_context::{ContextType, ServiceContext, ServiceContextConfig};
use crate::types::ssh::{
    SshConnectionConfig, SshConnectionState, SshConnectionStatus, SshLastConnection, SshTestResult,
};

use super::{ErrorResponse, error_json, success_json};

/// SSH context ID (single-connection model).
const SSH_CONTEXT_ID: &str = "ssh";

/// Resolve host request body.
#[derive(Deserialize)]
pub struct ResolveHostRequest {
    pub alias: String,
}

// ---------------------------------------------------------------------------
// Direct Json<T> response routes
// ---------------------------------------------------------------------------

/// POST /api/ssh/connect — Connect to SSH and switch context.
///
/// Establishes SSH connection, registers SSH context, performs context switch,
/// and emits context:changed via SSE. Cannot start watcher tasks (no AppHandle).
pub async fn ssh_connect(
    State(state): State<HttpState>,
    Json(body): Json<SshConnectionConfig>,
) -> Result<Json<SshConnectionStatus>, (StatusCode, Json<ErrorResponse>)> {
    // 1. Establish SSH connection
    let status = state
        .ssh_manager
        .write()
        .await
        .connect(body)
        .await
        .map_err(error_json)?;

    // If connection failed, return error status without switching context
    if matches!(status.state, SshConnectionState::Error) {
        return Ok(Json(status));
    }

    // 2. Build SSH ServiceContext
    let host = status.host.clone().unwrap_or_default();
    let remote_projects_path = status
        .remote_projects_path
        .clone()
        .unwrap_or_else(|| format!("/home/{}/.claude/projects", host));
    let remote_todos_path = PathBuf::from(&remote_projects_path)
        .parent()
        .map(|p| p.join("todos"))
        .unwrap_or_else(|| PathBuf::from("/tmp/claude-todos-ssh"));

    let fs_provider: Arc<dyn crate::infrastructure::FsProvider> = {
        let mgr = state.ssh_manager.read().await;
        mgr.get_provider().await
            .ok_or_else(|| error_json("SSH provider not available after connect"))?
    };

    let shared_cache = state.app_state.read().await.cache.clone();
    let ssh_context = ServiceContext::new(ServiceContextConfig {
        id: SSH_CONTEXT_ID.to_string(),
        context_type: ContextType::Ssh,
        projects_dir: PathBuf::from(&remote_projects_path),
        todos_dir: remote_todos_path,
        fs_provider,
        cache: Some(shared_cache),
    });

    // 3. If already on SSH context, tear down and switch back to local first
    // (mirrors Electron: destroy existing SSH context before creating new one)
    {
        let mut mgr = state.context_manager.write().await;
        if mgr.get_active_id() == SSH_CONTEXT_ID {
            log::info!("SSH connect (HTTP): already on SSH context, tearing down before reconnect");
            if let Some(ssh_ctx) = mgr.get(SSH_CONTEXT_ID) {
                ssh_ctx.read().await.stop_watcher_tasks().await;
            }
            let _ = mgr.switch("local");
            let _ = mgr.destroy_context(SSH_CONTEXT_ID).await;
        }
    }

    // 4. Register SSH context and switch
    {
        let mut mgr = state.context_manager.write().await;

        mgr.register_context(ssh_context).map_err(error_json)?;

        // Perform context switch
        let result = mgr.switch(SSH_CONTEXT_ID).map_err(error_json)?;
        log::info!(
            "SSH connect (HTTP): context switched {} -> {}",
            result.previous_id,
            result.current_id
        );

        // Stop old context's watcher tasks
        if let Some(old_ctx) = mgr.get(&result.previous_id) {
            old_ctx.read().await.stop_watcher_tasks().await;
        }

        // Start new context's watcher tasks
        if let Some(new_ctx) = mgr.get(&result.current_id) {
            let new = new_ctx.read().await;
            let config_manager = state.app_handle
                .state::<std::sync::Arc<crate::infrastructure::ConfigManager>>()
                .inner().clone();
            let notification_manager = state.app_handle
                .state::<std::sync::Arc<tokio::sync::RwLock<crate::infrastructure::NotificationManager>>>()
                .inner().clone();
            new.spawn_watcher_tasks(state.app_handle.clone(), config_manager, notification_manager).await;
        }

        // Emit context:changed via SSE
        let ctx_arc = mgr
            .get(&result.current_id)
            .ok_or_else(|| error_json("SSH context not found after switch"))
            .map_err(|(s, j)| (s, j))?;
        let info = ContextInfo::from_context(&*ctx_arc.read().await);

        state.broadcaster.send(BackendEvent::ContextChanged(info));
    }

    Ok(Json(status))
}

/// POST /api/ssh/disconnect — Disconnect SSH and switch back to local.
pub async fn ssh_disconnect(
    State(state): State<HttpState>,
) -> Result<Json<SshConnectionStatus>, (StatusCode, Json<ErrorResponse>)> {
    // Check if SSH context is currently active
    let is_ssh_active = {
        let mgr = state.context_manager.read().await;
        mgr.get_active_id() == SSH_CONTEXT_ID
    };

    if is_ssh_active {
        // Perform context switch lifecycle
        {
            let mut mgr = state.context_manager.write().await;

            let result = mgr.switch("local").map_err(error_json)?;
            log::info!(
                "SSH disconnect (HTTP): context switched {} -> {}",
                result.previous_id,
                result.current_id
            );

            // Stop old context's (SSH) watcher tasks
            if let Some(old_ctx) = mgr.get(&result.previous_id) {
                old_ctx.read().await.stop_watcher_tasks().await;
            }

            // Start local context's watcher tasks
            if let Some(new_ctx) = mgr.get(&result.current_id) {
                let new = new_ctx.read().await;
                let config_manager = state.app_handle
                    .state::<std::sync::Arc<crate::infrastructure::ConfigManager>>()
                    .inner().clone();
                let notification_manager = state.app_handle
                    .state::<std::sync::Arc<tokio::sync::RwLock<crate::infrastructure::NotificationManager>>>()
                    .inner().clone();
                new.spawn_watcher_tasks(state.app_handle.clone(), config_manager, notification_manager).await;
            }

            // Emit context:changed via SSE
            let ctx_arc = mgr
                .get(&result.current_id)
                .ok_or_else(|| error_json("Local context not found after switch"))
                .map_err(|(s, j)| (s, j))?;
            let info = ContextInfo::from_context(&*ctx_arc.read().await);

            // Destroy SSH context
            mgr.destroy_context(SSH_CONTEXT_ID)
                .await
                .map_err(error_json)?;

            state.broadcaster.send(BackendEvent::ContextChanged(info));
        }
    }

    // Disconnect SSH connection
    let status = state
        .ssh_manager
        .write()
        .await
        .disconnect()
        .await
        .map_err(error_json)?;

    Ok(Json(status))
}

/// GET /api/ssh/state — Get current SSH connection state.
pub async fn ssh_get_state(State(state): State<HttpState>) -> Json<SshConnectionStatus> {
    Json(state.ssh_manager.read().await.get_active_state().await)
}

/// POST /api/ssh/test — Test SSH connection configuration.
pub async fn ssh_test(
    State(state): State<HttpState>,
    Json(body): Json<SshConnectionConfig>,
) -> Json<SshTestResult> {
    let result = state.ssh_manager.read().await.test(&body).await;
    match result {
        Ok(test_result) => Json(test_result),
        Err(_) => Json(SshTestResult {
            success: false,
            error: Some("Test failed".to_string()),
        }),
    }
}

// ---------------------------------------------------------------------------
// Wrapped { success, data } response routes
// ---------------------------------------------------------------------------

/// GET /api/ssh/config-hosts — Get all SSH config host entries.
pub async fn ssh_get_config_hosts(
    State(state): State<HttpState>,
) -> (StatusCode, Json<serde_json::Value>) {
    let hosts = state.ssh_manager.read().await.get_config_hosts();
    success_json(hosts)
}

/// POST /api/ssh/resolve-host — Resolve a host alias from SSH config.
pub async fn ssh_resolve_host(
    State(state): State<HttpState>,
    Json(body): Json<ResolveHostRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let entry = state
        .ssh_manager
        .read()
        .await
        .resolve_host_config(&body.alias);
    success_json(entry)
}

// ---------------------------------------------------------------------------
// SSH connection persistence routes (wired to ConfigManager)
// ---------------------------------------------------------------------------

/// POST /api/ssh/save-last-connection — Save last SSH connection config.
pub async fn ssh_save_last_connection(
    State(state): State<HttpState>,
    Json(body): Json<SshLastConnection>,
) -> (StatusCode, Json<serde_json::Value>) {
    let connection_value = serde_json::json!({
        "lastConnection": {
            "host": body.host,
            "port": body.port,
            "username": body.username,
            "authMethod": body.auth_method,
            "privateKeyPath": body.private_key_path,
        }
    });
    match state.config_manager.update_config("ssh", connection_value) {
        Ok(_) => success_json(serde_json::Value::Null),
        Err(e) => (StatusCode::OK, Json(serde_json::json!({"success": false, "error": e.to_string()}))),
    }
}

/// GET /api/ssh/last-connection — Get last SSH connection config.
pub async fn ssh_get_last_connection(
    State(state): State<HttpState>,
) -> (StatusCode, Json<serde_json::Value>) {
    let config = state.config_manager.get_config();
    let last = config.ssh.as_ref().and_then(|s| s.last_connection.as_ref());
    let result = last.map(|c| SshLastConnection {
        host: c.host.clone(),
        port: c.port,
        username: c.username.clone(),
        auth_method: c.auth_method.clone(),
        private_key_path: c.private_key_path.clone(),
    });
    success_json(result)
}

// ---------------------------------------------------------------------------
// Router builder
// ---------------------------------------------------------------------------

/// Build SSH routes.
pub fn routes() -> axum::Router<HttpState> {
    axum::Router::new()
        .route("/api/ssh/connect", post(ssh_connect))
        .route("/api/ssh/disconnect", post(ssh_disconnect))
        .route("/api/ssh/state", get(ssh_get_state))
        .route("/api/ssh/test", post(ssh_test))
        .route("/api/ssh/config-hosts", get(ssh_get_config_hosts))
        .route("/api/ssh/resolve-host", post(ssh_resolve_host))
        .route(
            "/api/ssh/save-last-connection",
            post(ssh_save_last_connection),
        )
        .route("/api/ssh/last-connection", get(ssh_get_last_connection))
}
