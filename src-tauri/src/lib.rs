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
mod utils;

use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};
use tauri::{Emitter, Manager};
use commands::AppState;
use infrastructure::{ConfigManager, ContextManager, NotificationManager, SshConnectionManager};
use infrastructure::fs_provider::LocalFsProvider;
use infrastructure::service_context::{ContextType, ServiceContext, ServiceContextConfig};
use commands::tray::TrayIconManager;
use utils::get_projects_base_path;

/// 运行 Tauri 应用。
///
/// 初始化配置管理器、应用状态、通知管理器和上下文管理器，
/// 通过 ContextManager 启动本地上下文的文件监听器，
/// 并注册所有 IPC 命令处理函数。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  let config_manager = Arc::new(ConfigManager::new());
  let app_state = Arc::new(RwLock::new(AppState::new(config_manager.clone())));

  tauri::Builder::default()
    .plugin(tauri_plugin_dialog::init())
    .plugin(tauri_plugin_opener::init())
    .plugin(tauri_plugin_notification::init())
    .plugin(tauri_plugin_process::init())
    .plugin(tauri_plugin_autostart::Builder::new()
      .args(["--minimized"])
      .build())
    .manage(app_state.clone())
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
      let state = app_state.clone();

      // 非自动启动（无 --minimized 参数）时显示窗口
      let args: Vec<String> = std::env::args().collect();
      if !args.contains(&"--minimized".to_string()) {
        if let Some(window) = app.get_webview_window("main") {
          window.show().map_err(|e| e.to_string())?;
        }
      }

// Create and register TrayIconManager (needed for macOS dock hiding and window close interception)
      let tray_manager = std::sync::Mutex::new(TrayIconManager::new(app.handle().clone()));
      app.manage(tray_manager);

      // macOS: Create tray and hide Dock icon if config says so
      #[cfg(target_os = "macos")]
      {
        let hide_dock = {
          let state_guard = state.blocking_read();
          !state_guard.config_manager.get_config().general.show_dock_icon
        };
        if hide_dock {
          // Create tray FIRST, then hide dock
          let tray = app.state::<std::sync::Mutex<TrayIconManager>>();
          let _ = tray.lock().map(|mut t| t.create());
          use cocoa::appkit::{NSApplication, NSApplicationActivationPolicy};
          use cocoa::base::nil;
          unsafe {
            let ns_app = NSApplication::sharedApplication(nil);
            ns_app.setActivationPolicy_(
              NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
            );
            // Re-activate the app after switching to Accessory policy
            // macOS deactivates the app when switching to Accessory mode
            NSApplication::activateIgnoringOtherApps_(ns_app, true);
          }
        }
      }

      // 异步初始化应用状态
      tauri::async_runtime::spawn(async move {
        if let Err(e) = state.read().await.initialize().await {
          log::error!("Failed to initialize app state: {}", e);
        }
      });

      // 创建并注册 SSEBroadcaster
      let broadcaster = crate::http::sse::SSEBroadcaster::new();
      app.manage(broadcaster);

      // 创建并注册 HttpServerHandle（初始为 None，由 start 命令填充）
      app.manage(std::sync::Mutex::new(
        None::<crate::http::server::HttpServerHandle>,
      ));

      // 创建并注册 NotificationManager
      let notification_manager = NotificationManager::new(
        app.handle().clone(),
        config_manager.clone(),
      );
      let notification_manager = Arc::new(RwLock::new(notification_manager));
      app.manage(notification_manager.clone());

      // 异步初始化 NotificationManager
      let init_notification_mgr = notification_manager.clone();
      tauri::async_runtime::spawn(async move {
        init_notification_mgr.write().await.initialize().await;
        log::info!("NotificationManager initialized");
      });

      // ========== 创建 ContextManager 并注册本地上下文 ==========
      let context_manager = {
        let mut mgr = ContextManager::new();
        let local_context = ServiceContext::new(ServiceContextConfig {
          id: "local".to_string(),
          context_type: ContextType::Local,
          projects_dir: get_projects_base_path(),
          todos_dir: if let Some(home) = dirs::home_dir() {
            home.join(".claude").join("todos")
          } else {
            std::path::PathBuf::from("/tmp/claude-todos")
          },
          fs_provider: Arc::new(LocalFsProvider::new()),
        });
        mgr.register_context(local_context)
          .expect("Failed to register local context");

        // 启动本地上下文的 watcher 任务（在同步 setup 闭包中使用 block_on）
        let local_ctx = mgr.get("local").unwrap();
        let local = local_ctx.blocking_read();
        local.spawn_watcher_tasks(
          app.handle().clone(),
          config_manager.clone(),
          notification_manager.clone(),
        );

        mgr
      };
      let context_manager = Arc::new(RwLock::new(context_manager));
      app.manage(context_manager.clone());

      // ========== 创建并注册 SshConnectionManager ==========
      let ssh_manager_inner = SshConnectionManager::new();

      // 在包装为 Arc<RwLock<>> 之前获取 broadcast receiver（避免在 async 任务中持有读锁）
      let mut ssh_status_rx = ssh_manager_inner.subscribe();

      let ssh_manager = Arc::new(RwLock::new(ssh_manager_inner));
      app.manage(ssh_manager.clone());

      // 启动 SSH 状态事件转发任务
      let app_handle_for_ssh = app.handle().clone();
      tauri::async_runtime::spawn(async move {
        loop {
          match ssh_status_rx.recv().await {
            Ok(status) => {
              let event = crate::types::ssh::SshStatusChangedEvent { status };
              let _ = app_handle_for_ssh.emit("ssh:status", event);
            }
            Err(broadcast::error::RecvError::Closed) => break,
            Err(_) => continue,
          }
        }
      });

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
      commands::validation::validate_path,
      commands::validation::validate_mentions,
      commands::validation::scroll_to_line,
      commands::utility::open_path,
      commands::utility::open_external,
      commands::utility::get_zoom_factor,
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
      commands::updater::check_for_updates,
      commands::updater::download_and_install_update,
      commands::updater::install_update,
      commands::http_server::get_status,
      commands::http_server::start,
      commands::http_server::stop,
      commands::context::context_list,
      commands::context::context_active,
      commands::context::context_switch,
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
