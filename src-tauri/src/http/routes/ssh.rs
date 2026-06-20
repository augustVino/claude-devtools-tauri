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
// Wrapped { success, data } response routes
// ---------------------------------------------------------------------------
// Task 9: connect/disconnect/get_state/test 改用 success_json 包裹，对齐
// Electron http/ssh.ts:40,53,70 的 {success, data} / {success, error} 契约。
// 失败路径由 AppError IntoResponse 返回 {success:false, error}。

/// POST /api/ssh/connect
pub async fn ssh_connect(
    State(state): State<HttpState>,
    Json(body): Json<SshConnectionConfig>,
) -> Result<(axum::http::StatusCode, Json<serde_json::Value>), AppError> {
    let broadcaster = state.broadcaster.clone();
    // SshService::connect(config, broadcaster) — 无 AppHandle 参数
    // SshConnectResult 是 SshConnectionStatus 的类型别名
    let status = state.ssh_svc.connect(body, Some(&broadcaster)).await?;
    Ok(success_json(status))
}

/// POST /api/ssh/disconnect
pub async fn ssh_disconnect(
    State(state): State<HttpState>,
) -> Result<(axum::http::StatusCode, Json<serde_json::Value>), AppError> {
    let broadcaster = state.broadcaster.clone();
    // SshService::disconnect(broadcaster) — 仅 1 个参数
    let status = state.ssh_svc.disconnect(Some(&broadcaster)).await?;
    Ok(success_json(status))
}

/// GET /api/ssh/state
pub async fn ssh_get_state(
    State(state): State<HttpState>,
) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    // get_active_state 是 async 方法，需 .await；返回值非 Result，直接包裹
    success_json(state.ssh_svc.get_active_state().await)
}

/// POST /api/ssh/test
pub async fn ssh_test(
    State(state): State<HttpState>,
    Json(body): Json<SshConnectionConfig>,
) -> Result<(axum::http::StatusCode, Json<serde_json::Value>), AppError> {
    // test 接收 &SshConnectionConfig（引用）
    let result = state.ssh_svc.test(&body).await?;
    Ok(success_json(result))
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    /// Task 9: 直接调用 success_json 验证 SshConnectionStatus 包裹后的 JSON 结构。
    ///
    /// success_json 是 ssh.rs 4 个 handler（connect/disconnect/get_state/test）
    /// 共用的包裹函数，直接断言它的输出结构即可守护契约。SshService 是 trait +
    /// Arc<dyn>，完整 mock 需引入 mockall，不在本 Task 范围。
    #[test]
    fn test_success_json_wraps_data_field_for_ssh_status() {
        let status = SshConnectionStatus {
            state: crate::types::ssh::SshConnectionState::Disconnected,
            host: None,
            error: None,
            remote_projects_path: None,
        };
        let (code, json) = super::super::success_json(status);
        assert_eq!(code, StatusCode::OK);
        // json 是 axum::Json<serde_json::Value>，通过 .0 取出内部 Value
        let value = json.0;
        assert_eq!(value["success"], serde_json::Value::Bool(true));
        assert_eq!(value["data"]["state"], "disconnected");
        assert!(value["data"]["host"].is_null());
    }

    /// Task 9: SshTestResult 包裹后应该是 {success:true, data:{success, error?}}
    /// 嵌套结构。守护嵌套 success 字段的契约（前端 httpClient.test 依赖此结构）。
    #[test]
    fn test_success_json_wraps_data_field_for_ssh_test_result() {
        let result = SshTestResult {
            success: false,
            error: Some("auth failed".to_string()),
        };
        let (code, json) = super::super::success_json(result);
        assert_eq!(code, StatusCode::OK);
        let value = json.0;
        // 外层 success 是包裹标记（永远 true）
        assert_eq!(value["success"], serde_json::Value::Bool(true));
        // 内层 success 才是真实测试结果
        assert_eq!(value["data"]["success"], serde_json::Value::Bool(false));
        assert_eq!(
            value["data"]["error"],
            serde_json::Value::String("auth failed".to_string())
        );
    }
}
