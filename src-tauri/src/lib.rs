//! Tauri 应用库入口模块。
//!
//! 初始化应用状态、注册插件和 IPC 命令、启动后台文件监听器，
//! 并构建 Tauri 应用实例。

mod analysis;
mod commands;
mod constants;
mod discovery;
mod error;
mod events;
mod http;
mod infrastructure;
mod parsing;
mod types;
mod services;
mod utils;

use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};
use tauri::{Emitter, Manager};
use commands::AppState;
use infrastructure::{ConfigManager, ContextManager, NotificationManager, SshConnectionManager};
use infrastructure::fs_provider::LocalFsProvider;
use infrastructure::service_context::{ContextType, ServiceContext, ServiceContextConfig};
use infrastructure::app_bootstrap::AppBootstrap;
use commands::tray::TrayIconManager;
use utils::{get_default_claude_base_path, get_projects_base_path, get_todos_base_path, set_claude_root_override};

/// 运行 Tauri 应用。
///
/// 初始化配置管理器、应用状态、通知管理器和上下文管理器，
/// 通过 ContextManager 启动本地上下文的文件监听器，
/// 并注册所有 IPC 命令处理函数。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  let config_manager = Arc::new(ConfigManager::new());
  let shared_cache = infrastructure::DataCache::new();
  let app_state = Arc::new(RwLock::new(AppState::new(config_manager.clone(), shared_cache.clone())));

  // ========== Pre-create Domain Services (registered inside setup) ==========
  let fs_provider: Arc<dyn infrastructure::fs_provider::FsProvider> = Arc::new(infrastructure::fs_provider::LocalFsProvider::new());

  let project_service = Arc::new(services::ProjectService::new(
      fs_provider.clone(),
      get_projects_base_path(),
      get_todos_base_path(),
  ));

  let search_service = Arc::new(services::SearchService::new(
      get_projects_base_path(),
      get_todos_base_path(),
      fs_provider.clone(),
  ));

  let session_repo: Arc<dyn infrastructure::session_repository::SessionRepository> =
      Arc::new(infrastructure::local_session_repository::LocalSessionRepository::new(
          fs_provider.clone(),
          get_projects_base_path(),
          get_default_claude_base_path(),
      ));

  let session_service = Arc::new(services::SessionService::new(
      fs_provider,
      shared_cache.clone(),
      get_projects_base_path(),
      get_todos_base_path(),
      config_manager.clone(),
      project_service.clone(),
      session_repo,
  ));

  // Zoom factor state: track zoom since Tauri v2 has set_zoom() but no zoom() getter.
  // Store f64 as bits in AtomicU64 for lock-free concurrent access.
  let zoom_factor: Arc<AtomicU64> = Arc::new(AtomicU64::new(1.0f64.to_bits()));

  tauri::Builder::default()
    .plugin(tauri_plugin_dialog::init())
    .plugin(tauri_plugin_opener::init())
    .plugin(tauri_plugin_notification::init())
    .plugin(tauri_plugin_process::init())
    .plugin(tauri_plugin_autostart::Builder::new()
      .args(["--minimized"])
      .build())
    .plugin(tauri_plugin_updater::Builder::new().build())
    .manage(app_state.clone())
    .manage(zoom_factor)
    .on_window_event(|window, event| {
      if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        #[cfg(target_os = "macos")]
        {
          // Use try_state() because this handler is registered before .setup()
          // where TrayIconManager is actually managed
          let tray = window.app_handle().try_state::<std::sync::Mutex<TrayIconManager>>();
          if let Some(tray) = tray {
            if let Ok(tray) = tray.lock() {
              if !tray.is_dock_visible() {
                log::info!("Hiding window to tray (dock hidden)");
                let _ = window.hide();
                api.prevent_close();
              }
            }
          }
        }
      }
    })
    .setup(move |app| {
      // 1. 初始化配置（必须在最前面执行）
      AppBootstrap::init_config(&config_manager)?;

      // 2. 设置 claude root 路径覆盖
      AppBootstrap::set_claude_root(&config_manager);

      // 3. 注册核心 managed state
      app.manage(config_manager.clone());
      app.manage(session_service.clone());
      app.manage(project_service.clone());
      app.manage(search_service.clone());

      // 4. 显示窗口（非 --minimized 模式）
      AppBootstrap::show_window_if_needed(app.handle())?;

      // 5. Tray 图标
      let tray_manager = std::sync::Mutex::new(TrayIconManager::new(app.handle().clone()));
      app.manage(tray_manager);

      // 6. macOS Dock 隐藏
      #[cfg(target_os = "macos")]
      AppBootstrap::hide_dock_if_needed(app.handle(), &app_state);

      // 7. SSE + HttpServerHandle
      let broadcaster = crate::http::sse::SSEBroadcaster::new();
      app.manage(broadcaster);
      app.manage(std::sync::Mutex::new(
        None::<crate::http::server::HttpServerHandle>,
      ));

      // 8. NotificationManager
      let last_shown_error = std::sync::Arc::new(std::sync::Mutex::new(None::<crate::types::config::DetectedError>));
      let notification_manager = NotificationManager::new(
        app.handle().clone(),
        config_manager.clone(),
        last_shown_error.clone(),
      );
      let notification_manager = Arc::new(RwLock::new(notification_manager));
      app.manage(notification_manager.clone());
      app.manage(last_shown_error);

      // 异步初始化 NotificationManager
      let init_notification_mgr = notification_manager.clone();
      tauri::async_runtime::spawn(async move {
        init_notification_mgr.write().await.initialize().await;
        log::info!("NotificationManager initialized");
      });

      // 9. ContextManager + 本地上下文 + Watcher
      let context_manager = AppBootstrap::setup_local_context(
        app.handle(),
        &config_manager,
        &shared_cache,
        &notification_manager,
      );
      app.manage(context_manager.clone());

      // 10. SshConnectionManager + SSH 状态转发
      let ssh_manager_inner = SshConnectionManager::new();

      // 在包装为 Arc<RwLock<>> 之前获取 broadcast receiver（避免在 async 任务中持有读锁）
      let mut ssh_status_rx = ssh_manager_inner.subscribe();

      let ssh_manager = Arc::new(RwLock::new(ssh_manager_inner));
      app.manage(ssh_manager.clone());

      // 启动 SSH 状态事件转发任务
      let app_handle_for_ssh = app.handle().clone();
      let ssh_broadcaster = app_handle_for_ssh.state::<crate::http::sse::SSEBroadcaster>().inner().clone();
      tauri::async_runtime::spawn(async move {
        loop {
          match ssh_status_rx.recv().await {
            Ok(status) => {
              let event = crate::types::ssh::SshStatusChangedEvent { status: status.clone() };
              let _ = app_handle_for_ssh.emit("ssh:status", event);
              // Bridge to SSE broadcaster for HTTP-only clients
              ssh_broadcaster.send(crate::http::sse::BackendEvent::SshStatusChanged(status));
            }
            Err(broadcast::error::RecvError::Closed) => break,
            Err(_) => continue,
          }
        }
      });

      // Auto-start HTTP server if enabled in config
      infrastructure::app_bootstrap::AppBootstrap::auto_start_http_server(
        app.handle(),
        &config_manager,
        &app_state,
        &session_service,
        &project_service,
        &search_service,
      )?;

      // Debug 模式下启用日志插件
      if cfg!(debug_assertions) {
        app.handle().plugin(
          tauri_plugin_log::Builder::default()
            .level(log::LevelFilter::Info)
            .build(),
        )?;
      }
      Ok(())
    })
    .invoke_handler(tauri::generate_handler![
      commands::window::minimize,
      commands::window::maximize,
      commands::window::close,
      commands::window::is_maximized,
      commands::window::relaunch,
      commands::window::set_dock_visible,
      commands::version::get_app_version,
      commands::sessions::get_sessions,
      commands::sessions::get_session_detail,
      commands::sessions::get_session_metrics,
      commands::sessions::get_sessions_paginated,
      commands::sessions::get_sessions_by_ids,
      commands::sessions::get_session_groups,
      commands::sessions::get_waterfall_data,
      commands::sessions::delete_session,
      commands::sessions::get_projects,
      commands::config::get_config,
      commands::config::update_config,
      commands::config::add_ignore_regex,
      commands::config::remove_ignore_regex,
      commands::config::pin_session,
      commands::config::unpin_session,
      commands::config::hide_session,
      commands::config::unhide_session,
      commands::config::snooze,
      commands::config::clear_snooze,
      commands::config::add_trigger,
      commands::config::update_trigger,
      commands::config::remove_trigger,
      commands::config::get_triggers,
      commands::config::test_trigger,
      commands::config::open_in_editor,
      commands::config::get_claude_root_info,
      commands::config::add_ignore_repository,
      commands::config::remove_ignore_repository,
      commands::config::hide_sessions,
      commands::config::unhide_sessions,
      commands::config::check_projects_dir_exists,
      commands::search::search_sessions,
      commands::search::search_all_projects,
      commands::search::find_session_by_id,
      commands::search::find_sessions_by_partial_id,
      commands::validation::validate_path,
      commands::validation::validate_mentions,
      commands::validation::scroll_to_line,
      commands::utility::open_path,
      commands::utility::open_external,
      commands::utility::get_zoom_factor,
      commands::utility::set_zoom_factor,
      commands::utility::read_claude_md_files,
      commands::utility::read_directory_claude_md,
      commands::utility::read_mentioned_file,
      commands::utility::read_agent_configs,
      commands::utility::write_text_file,
      commands::projects::get_repository_groups,
      commands::projects::get_worktree_sessions,
      commands::subagents::get_subagent_detail,
      commands::notifications::get_notifications,
      commands::notifications::mark_notification_read,
      commands::notifications::mark_all_notifications_read,
      commands::notifications::delete_notification,
      commands::notifications::clear_notifications,
      commands::notifications::get_notification_count,
      commands::notifications::get_notification_stats,
      commands::notifications::handle_notification_click,
      commands::http_server::get_status,
      commands::http_server::start,
      commands::http_server::stop,
      commands::context::context_list,
      commands::context::context_active,
      commands::context::context_switch,
      commands::ssh_connect,
      commands::ssh_disconnect,
      commands::ssh_get_state,
      commands::ssh_test,
      commands::ssh_get_config_hosts,
      commands::ssh_resolve_host,
      commands::ssh_save_last_connection,
      commands::ssh_get_last_connection,
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
