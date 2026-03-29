//! HTTP 服务器控制命令。

use std::sync::{Arc, Mutex};
use tauri::{command, AppHandle, Manager, State};
use tokio::sync::RwLock;

use crate::commands::search::create_searcher_state;
use crate::commands::AppState;
use crate::http::server::{self, HttpServerHandle, HttpServerStatus};
use crate::http::sse::SSEBroadcaster;
use crate::http::state::HttpState;
use crate::infrastructure::fs_provider::LocalFsProvider;
use crate::infrastructure::{ContextManager, NotificationManager, SshConnectionManager};
use crate::utils::get_projects_base_path;

/// 获取 HTTP 服务器状态。
#[command]
pub async fn get_status(app: AppHandle) -> Result<HttpServerStatus, String> {
    let handle = app.state::<Mutex<Option<HttpServerHandle>>>();
    let guard = handle.lock().map_err(|e| e.to_string())?;
    match guard.as_ref() {
        Some(h) => Ok(HttpServerStatus {
            running: true,
            port: h.port,
        }),
        None => Ok(HttpServerStatus {
            running: false,
            port: 3456,
        }),
    }
}

/// 启动 HTTP 服务器。
#[command]
pub async fn start(
    app: AppHandle,
    state: State<'_, Arc<RwLock<AppState>>>,
) -> Result<HttpServerStatus, String> {
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
    {
        let handle_guard = app.state::<Mutex<Option<HttpServerHandle>>>();
        let mut handle = handle_guard.lock().map_err(|e| e.to_string())?;

        // Already running?
        if handle.is_some() {
            let existing = handle.as_ref().unwrap();
            return Ok(HttpServerStatus {
                running: true,
                port: existing.port,
            });
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

        let projects_dir = get_projects_base_path();
        let searcher = Arc::new(create_searcher_state(projects_dir, Arc::new(LocalFsProvider::new())));

        let http_state = HttpState {
            app_state: state.inner().clone(),
            broadcaster,
            notification_manager,
            searcher,
            context_manager,
            ssh_manager: app
                .state::<Arc<RwLock<SshConnectionManager>>>()
                .inner()
                .clone(),
        };

        // 前端构建产物目录：CARGO_MANIFEST_DIR/../dist
        let dist_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("dist");

        let new_handle = server::spawn_http_server(http_state, preferred_port, dist_dir)?;
        let port = new_handle.port;

        *handle = Some(new_handle);

        log::info!("HTTP server started on port {}", port);
        Ok(HttpServerStatus {
            running: true,
            port,
        })
    }
}

/// 停止 HTTP 服务器。
#[command]
pub async fn stop(app: AppHandle) -> Result<HttpServerStatus, String> {
    let handle_guard = app.state::<Mutex<Option<HttpServerHandle>>>();
    let mut handle = handle_guard.lock().map_err(|e| e.to_string())?;

    if let Some(h) = handle.take() {
        h.shutdown.cancel();
        log::info!("HTTP server stopped");
    }

    Ok(HttpServerStatus {
        running: false,
        port: 3456,
    })
}
