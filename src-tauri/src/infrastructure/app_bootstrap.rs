//! 应用启动编排 — 从 lib.rs 中提取的组装逻辑。

use std::sync::Arc;
use tokio::sync::RwLock;

use tauri::Manager;
use crate::commands::AppState;
use crate::commands::tray::TrayIconManager;
use crate::infrastructure::{ConfigManager, ContextManager, NotificationManager, SshConnectionManager};
use crate::infrastructure::fs_provider::LocalFsProvider;
use crate::infrastructure::service_context::{ContextType, ServiceContext, ServiceContextConfig};
use crate::utils::{get_projects_base_path, get_todos_base_path, set_claude_root_override};

/// 应用启动编排器。
pub struct AppBootstrap;

impl AppBootstrap {
    /// 初始化 ConfigManager（必须在最前面执行）。
    pub fn init_config(config_manager: &Arc<ConfigManager>) -> Result<(), String> {
        tauri::async_runtime::block_on(config_manager.initialize())
            .map_err(|e| format!("Failed to initialize config: {e}"))
    }

    /// 设置全局 claude root 路径覆盖。
    pub fn set_claude_root(config_manager: &Arc<ConfigManager>) {
        set_claude_root_override(tauri::async_runtime::block_on(config_manager.get_config()).general.claude_root_path.clone());
    }

    /// 非 debug 模式时显示窗口。
    pub fn show_window_if_needed(app: &tauri::AppHandle) -> Result<(), String> {
        let args: Vec<String> = std::env::args().collect();
        if !args.contains(&"--minimized".to_string()) {
            if let Some(window) = app.get_webview_window("main") {
                window.show().map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    /// macOS Dock 图标隐藏逻辑。
    #[cfg(target_os = "macos")]
    pub fn hide_dock_if_needed(
        app: &tauri::AppHandle,
        state: &Arc<RwLock<AppState>>,
    ) {
        let hide_dock = {
            let state_guard = state.blocking_read();
            !tauri::async_runtime::block_on(state_guard.config_manager.get_config()).general.show_dock_icon
        };
        if hide_dock {
            let tray = app.state::<std::sync::Mutex<TrayIconManager>>();
            let _ = tray.lock().map(|mut t| t.create());
            use cocoa::appkit::{NSApplication, NSApplicationActivationPolicy};
            use cocoa::base::nil;
            unsafe {
                let ns_app = NSApplication::sharedApplication(nil);
                ns_app.setActivationPolicy_(
                    NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
                );
                NSApplication::activateIgnoringOtherApps_(ns_app, true);
            }
        }
    }

    /// 自动启动 HTTP server（如果配置启用）。
    ///
    /// 完整移植自 lib.rs L253-320 的 HTTP server 启动逻辑。
    pub fn auto_start_http_server(
        app: &tauri::AppHandle,
        config_manager: &Arc<ConfigManager>,
        app_state: &Arc<RwLock<AppState>>,
        session_service: &Arc<dyn crate::services::SessionService>,
        project_service: &Arc<dyn crate::services::ProjectService>,
        search_service: &Arc<dyn crate::services::SearchServiceFull>,
    ) -> Result<(), String> {
        let http_config = tauri::async_runtime::block_on(config_manager.get_config()).http_server.clone();
        if let Some(ref cfg) = http_config {
            if cfg.enabled {
                let port = cfg.port;
                let handle_guard = app.state::<std::sync::Mutex<Option<crate::http::server::HttpServerHandle>>>();
                let mut handle = match handle_guard.lock() {
                    Ok(g) => g,
                    Err(e) => {
                        log::error!("Failed to acquire HTTP server handle lock: {e}");
                        return Err(format!("Failed to acquire HTTP server handle lock: {e}"));
                    }
                };

                if handle.is_none() {
                    let broadcaster = app.state::<crate::http::sse::SSEBroadcaster>().inner().clone();
                    let notification_manager = app
                        .state::<Arc<RwLock<NotificationManager>>>()
                        .inner()
                        .clone();
                    let context_manager = app
                        .state::<Arc<RwLock<ContextManager>>>()
                        .inner()
                        .clone();
                    let http_state = crate::http::state::HttpState {
                        app_handle: app.clone(),
                        app_state: app_state.clone(),
                        broadcaster,
                        config_manager: config_manager.clone(),
                        notification_manager,
                        context_manager,
                        ssh_manager: app
                            .state::<Arc<RwLock<SshConnectionManager>>>()
                            .inner()
                            .clone(),
                        session_service: session_service.clone(),
                        project_service: project_service.clone(),
                        search_service: search_service.clone(),
                    };

                    let dist_dir = std::env::var("RENDERER_PATH")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|_| {
                            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                                .join("..")
                                .join("dist")
                        });

                    match crate::http::server::spawn_http_server(http_state, port, dist_dir) {
                        Ok(new_handle) => {
                            log::info!("HTTP server auto-started on port {} (enabled in config)", new_handle.port);
                            *handle = Some(new_handle);
                        }
                        Err(e) => log::error!("Failed to auto-start HTTP server: {e}"),
                    }
                }
            }
        }
        Ok(())
    }

    /// 创建并注册本地 ContextManager（含 watcher 任务启动）。
    pub fn setup_local_context(
        app: &tauri::AppHandle,
        config_manager: &Arc<ConfigManager>,
        shared_cache: &crate::infrastructure::DataCache,
        notification_manager: &Arc<RwLock<NotificationManager>>,
    ) -> Arc<RwLock<ContextManager>> {
        let mut mgr = ContextManager::new();
        let local_context = ServiceContext::new(ServiceContextConfig {
            id: "local".to_string(),
            context_type: ContextType::Local,
            projects_dir: get_projects_base_path(),
            todos_dir: get_todos_base_path(),
            fs_provider: Arc::new(LocalFsProvider::new()),
            cache: Some(shared_cache.clone()),
        });
        mgr.register_context(local_context)
            .expect("Failed to register local context");

        // 启动本地上下文的 watcher 任务
        let local_ctx = mgr.get("local").unwrap();
        let local = local_ctx.blocking_read();
        tauri::async_runtime::block_on(
            local.spawn_watcher_tasks(
                app.clone(),
                config_manager.clone(),
                notification_manager.clone(),
            )
        );

        let context_manager = Arc::new(RwLock::new(mgr));
        context_manager.clone()
    }
}
