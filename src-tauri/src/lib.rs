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

use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use tauri::{Emitter, Manager};
use commands::AppState;
use error::error_detector::ErrorDetector;
use infrastructure::{ConfigManager, FileWatcher, NotificationManager};
use commands::tray::TrayIconManager;
use utils::{get_projects_base_path, is_subagent_file};

/// 运行 Tauri 应用。
///
/// 初始化配置管理器、应用状态、通知管理器，
/// 启动三个并发文件监听器（主监听器、错误检测管道、Todo 监听器），
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

      // ========== 主文件监听器：监听 JSONL/JSON 文件变更 ==========
      let watcher_app_handle = app.handle().clone();
      let watcher_state = app_state.clone();
      tauri::async_runtime::spawn(async move {
        let mut watcher = FileWatcher::new();
        let projects_path = get_projects_base_path();

        // 目录不存在时自动创建
        if !projects_path.exists() {
          if let Err(e) = tokio::fs::create_dir_all(&projects_path).await {
            log::error!("Failed to create projects directory: {}", e);
            return;
          }
        }

        // 启动文件监听
        match watcher.watch(&projects_path).await {
          Ok(()) => {
            log::info!("FileWatcher started successfully");
            let mut receiver = watcher.receiver();

            // 处理文件变更事件
            while let Ok(event) = receiver.recv().await {
              // 失效缓存，确保后续 getSessionDetail 重新解析文件
              if let (Some(pid), Some(sid)) = (&event.project_id, &event.session_id) {
                let app_state = watcher_state.read().await;
                app_state.cache.invalidate_session(pid, sid).await;
              }
              events::emit_file_change(&watcher_app_handle, event);
            }
          }
          Err(e) => {
            log::error!("Failed to start FileWatcher: {}", e);
          }
        }
      });

      // ========== 错误检测管道：第二个 FileWatcher 监听 .jsonl 变更 ==========
      let pipeline_handle = app.handle().clone();
      let pipeline_notification_mgr = notification_manager.clone();
      let pipeline_config = config_manager.clone(); // 共享同一个 ConfigManager 实例（已由命令层初始化）
      tauri::async_runtime::spawn(async move {
        let detector = ErrorDetector::new(pipeline_config);

        let mut pipeline_watcher = FileWatcher::new();
        let projects_path = get_projects_base_path();

        if !projects_path.exists() {
          // 目录可能尚未创建；静默跳过（主监听器负责创建）
          return;
        }

        match pipeline_watcher.watch(&projects_path).await {
          Ok(()) => {
            log::info!("Error detection pipeline: watcher started");
            let mut receiver = pipeline_watcher.receiver();

            while let Ok(event) = receiver.recv().await {
              let path = Path::new(&event.path);

              // 仅处理 .jsonl 文件，跳过子 Agent 文件（由父会话处理）
              if path.extension().map(|e| e != "jsonl").unwrap_or(true) {
                continue;
              }
              if is_subagent_file(&event.path) {
                continue;
              }

              let session_id = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
              let project_id = event
                .project_id
                .clone()
                .unwrap_or_default();

              // 解析会话文件并执行错误检测
              let messages =
                crate::parsing::jsonl_parser::parse_jsonl_file(path).await;

              if messages.is_empty() {
                continue;
              }

              let errors = detector
                .detect_errors(
                  &messages,
                  &session_id,
                  &project_id,
                  &event.path,
                )
                .await;

              // 将检测到的错误发送给 NotificationManager
              let mgr = pipeline_notification_mgr.read().await;
              for detected_error in errors {
                events::emit_error_detected(&pipeline_handle, &detected_error);
                // add_error 内部会发射 notification:new 和 notification:updated 事件
                let _ = mgr.add_error(detected_error).await;
              }
            }
          }
          Err(e) => {
            log::error!("Error detection pipeline: failed to start watcher: {}", e);
          }
        }
      });

      // ========== Todo 文件监听器：监听 ~/.claude/todos/ 的清单变更 ==========
      let todo_app_handle = app.handle().clone();
      tauri::async_runtime::spawn(async move {
        let mut todo_watcher = FileWatcher::new();

        let todos_path = if let Some(home) = dirs::home_dir() {
          home.join(".claude").join("todos")
        } else {
          return;
        };

        if !todos_path.exists() {
          if let Err(e) = tokio::fs::create_dir_all(&todos_path).await {
            log::error!("Failed to create todos directory: {}", e);
            return;
          }
        }

        match todo_watcher.watch(&todos_path).await {
          Ok(()) => {
            log::info!("Todo FileWatcher started successfully");
            let mut receiver = todo_watcher.receiver();

            while let Ok(event) = receiver.recv().await {
              let session_id = Path::new(&event.path)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
              events::emit_todo_change(
                &todo_app_handle,
                events::TodoChangeEvent { session_id },
              );
            }
          }
          Err(e) => {
            log::error!("Failed to start todo FileWatcher: {}", e);
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
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
