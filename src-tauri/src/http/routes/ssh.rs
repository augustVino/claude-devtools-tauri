//! SSH HTTP routes — REST endpoints for SSH connection lifecycle.
//!
//! Business logic delegated to SshService. Error handling uses AppError IntoResponse.
//! All services accessed via HttpState fields (Axum single-State constraint).

use axum::{
    extract::State,
    routing::{get, post},
    Json,
};
use serde::Deserialize;

use crate::error::AppError;
use crate::http::state::HttpState;
use crate::types::ssh::{
    SshConnectionConfig, SshConnectionStatus, SshLastConnection, SshTestResult,
};

use super::success_json;

/// Resolve host request body.
#[derive(Deserialize)]
pub struct ResolveHostRequest {
    pub alias: String,
}

// ---------------------------------------------------------------------------
// Direct Json<T> response routes (using AppError IntoResponse)
// ---------------------------------------------------------------------------

/// POST /api/ssh/connect
pub async fn ssh_connect(
    State(state): State<HttpState>,
    Json(body): Json<SshConnectionConfig>,
) -> Result<Json<SshConnectionStatus>, AppError> {
    let broadcaster = state.broadcaster.clone();
    // SshService::connect(config, broadcaster) — 无 AppHandle 参数
    // SshConnectResult 是 SshConnectionStatus 的类型别名
    let status = state.ssh_svc.connect(body, Some(&broadcaster)).await?;
    Ok(Json(status))
}

/// POST /api/ssh/disconnect
pub async fn ssh_disconnect(
    State(state): State<HttpState>,
) -> Result<Json<SshConnectionStatus>, AppError> {
    let broadcaster = state.broadcaster.clone();
    // SshService::disconnect(broadcaster) — 仅 1 个参数
    let status = state.ssh_svc.disconnect(Some(&broadcaster)).await?;
    Ok(Json(status))
}

/// GET /api/ssh/state
pub async fn ssh_get_state(State(state): State<HttpState>) -> Json<SshConnectionStatus> {
    // get_active_state 是 async 方法，需 .await；返回值非 Result，直接包装
    Json(state.ssh_svc.get_active_state().await)
}

/// POST /api/ssh/test
pub async fn ssh_test(
    State(state): State<HttpState>,
    Json(body): Json<SshConnectionConfig>,
) -> Result<Json<SshTestResult>, AppError> {
    // test 接收 &SshConnectionConfig（引用）
    let result = state.ssh_svc.test(&body).await?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// Wrapped { success, data } response routes (unchanged thin CRUD wrappers)
// ---------------------------------------------------------------------------

/// GET /api/ssh/config-hosts
pub async fn ssh_get_config_hosts(
    State(state): State<HttpState>,
) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    // get_config_hosts 是 async，需要 .await
    success_json(state.ssh_svc.get_config_hosts().await)
}

/// POST /api/ssh/resolve-host
pub async fn ssh_resolve_host(
    State(state): State<HttpState>,
    Json(body): Json<ResolveHostRequest>,
) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    // resolve_host_config 是 async，需要 .await
    success_json(state.ssh_svc.resolve_host_config(&body.alias).await)
}

// ---------------------------------------------------------------------------
// SSH connection persistence routes (wired to ConfigManager, unchanged)
// ---------------------------------------------------------------------------

/// POST /api/ssh/save-last-connection
pub async fn ssh_save_last_connection(
    State(state): State<HttpState>,
    Json(body): Json<SshLastConnection>,
) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    let connection_value = serde_json::json!({
        "lastConnection": {
            "host": body.host,
            "port": body.port,
            "username": body.username,
            "authMethod": body.auth_method,
            "privateKeyPath": body.private_key_path,
        }
    });
    match state
        .config_manager
        .update_config("ssh", connection_value)
        .await
    {
        Ok(_) => success_json(serde_json::Value::Null),
        Err(e) => (
            axum::http::StatusCode::OK,
            Json(serde_json::json!({"success": false, "error": e.to_string()})),
        ),
    }
}

/// GET /api/ssh/last-connection
pub async fn ssh_get_last_connection(
    State(state): State<HttpState>,
) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    let config = state.config_manager.get_config().await;
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
