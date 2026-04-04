//! HTTP 服务器控制命令。

use std::sync::{Arc, Mutex};
use tauri::{command, AppHandle, Manager, State};
use tokio::sync::RwLock;

use crate::commands::AppState;
use crate::http::server::{self, HttpServerHandle, HttpServerStatus};
use crate::http::sse::SSEBroadcaster;
use crate::http::state::HttpState;
use crate::infrastructure::{ConfigManager, ContextManager, NotificationManager, SshConnectionManager};
use crate::services::{ProjectService, SearchService, SessionService};

/// IPC 响应包装 — 与 Electron 格式对齐
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcResponse<T: serde::Serialize> {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl<T: serde::Serialize> IpcResponse<T> {
    fn ok(data: T) -> Self {
        Self { success: true, data: Some(data), error: None }
    }
}

impl IpcResponse<()> {
    fn err(msg: impl Into<String>) -> IpcResponse<serde_json::Value> {
        IpcResponse { success: false, data: None, error: Some(msg.into()) }
    }
}

/// 获取 HTTP 服务器状态。
#[command]
pub async fn get_status(app: AppHandle) -> Result<IpcResponse<HttpServerStatus>, String> {
    let handle = app.state::<Mutex<Option<HttpServerHandle>>>();
    let guard = handle.lock().map_err(|e| e.to_string())?;
    match guard.as_ref() {
        Some(h) => Ok(IpcResponse::ok(HttpServerStatus { running: true, port: h.port })),
        None => Ok(IpcResponse::ok(HttpServerStatus { running: false, port: 3456 })),
    }
}

/// 启动 HTTP 服务器。
#[command]
pub async fn start(
    app: AppHandle,
    state: State<'_, Arc<RwLock<AppState>>>,
) -> Result<IpcResponse<HttpServerStatus>, String> {
    // Read config port first (before acquiring std::sync::Mutex to avoid holding it across await)
    let preferred_port = {
        let state_read = state.read().await;
        state_read
            .config_manager
            .get_config()
            .http_server
            .as_ref()
            .map(|c| c.port)
            .unwrap_or(3456)
    };

    // Acquire std::sync::Mutex — must not hold this across any .await point
    let port = {
        let handle_guard = app.state::<Mutex<Option<HttpServerHandle>>>();
        let mut handle = handle_guard.lock().map_err(|e| e.to_string())?;

        // Already running?
        if handle.is_some() {
            let existing = handle.as_ref().unwrap();
            return Ok(IpcResponse::ok(HttpServerStatus {
                running: true,
                port: existing.port,
            }));
        }

        let broadcaster = app.state::<SSEBroadcaster>().inner().clone();
        let notification_manager = app
            .state::<Arc<RwLock<NotificationManager>>>()
            .inner()
            .clone();

        let context_manager = app
            .state::<Arc<RwLock<ContextManager>>>()
            .inner()
            .clone();

        let http_state = HttpState {
            app_handle: app.clone(),
            app_state: state.inner().clone(),
            broadcaster,
            config_manager: app
                .state::<Arc<ConfigManager>>()
                .inner()
                .clone(),
            notification_manager,
            context_manager,
            ssh_manager: app
                .state::<Arc<RwLock<SshConnectionManager>>>()
                .inner()
                .clone(),
            session_service: app.state::<Arc<SessionService>>().inner().clone(),
            project_service: app.state::<Arc<ProjectService>>().inner().clone(),
            search_service: app.state::<Arc<SearchService>>().inner().clone(),
        };

        // 前端构建产物目录：RENDERER_PATH 环境变量优先，fallback 到 CARGO_MANIFEST_DIR/../dist
        let dist_dir = std::env::var("RENDERER_PATH")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("..")
                    .join("dist")
            });

        let new_handle = server::spawn_http_server(http_state, preferred_port, dist_dir)?;
        let port = new_handle.port;

        *handle = Some(new_handle);

        log::info!("HTTP server started on port {}", port);
        port
    }; // Mutex guard dropped

    // Persist enabled state to config
    let config_mgr = app.state::<Arc<ConfigManager>>().inner().clone();
    if let Err(e) = config_mgr.update_config("httpServer", serde_json::json!({"enabled": true, "port": port})) {
        log::error!("Failed to persist httpServer.enabled=true: {e}");
    }

    Ok(IpcResponse::ok(HttpServerStatus {
        running: true,
        port,
    }))
}

/// 停止 HTTP 服务器。
#[command]
pub async fn stop(app: AppHandle) -> Result<IpcResponse<HttpServerStatus>, String> {
    let stopped = {
        let handle_guard = app.state::<Mutex<Option<HttpServerHandle>>>();
        let mut handle = handle_guard.lock().map_err(|e| e.to_string())?;

        if let Some(h) = handle.take() {
            h.shutdown.cancel();
            log::info!("HTTP server stopped");
            true
        } else {
            false
        }
    }; // Mutex guard dropped

    // Persist disabled state to config
    if stopped {
        let config_mgr = app.state::<Arc<ConfigManager>>().inner().clone();
        if let Err(e) = config_mgr.update_config("httpServer", serde_json::json!({"enabled": false})) {
            log::error!("Failed to persist httpServer.enabled=false: {e}");
        }
    }

    Ok(IpcResponse::ok(HttpServerStatus {
        running: false,
        port: 3456,
    }))
}
