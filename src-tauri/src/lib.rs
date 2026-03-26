mod analysis;
mod commands;
mod constants;
mod discovery;
mod error;
mod events;
mod infrastructure;
mod parsing;
mod types;
mod utils;

use std::path::Path;
use std::sync::{Arc, RwLock as StdRwLock};
use tokio::sync::RwLock;
use tauri::{Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;
use commands::AppState;
use commands::updater::UpdaterStatus;
use error::error_detector::ErrorDetector;
use infrastructure::{ConfigManager, FileWatcher, NotificationManager};
use utils::{get_projects_base_path, is_subagent_file};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  let config_manager = Arc::new(ConfigManager::new());
  let app_state = Arc::new(RwLock::new(AppState::new(config_manager.clone())));

  tauri::Builder::default()
    .plugin(tauri_plugin_dialog::init())
    .plugin(tauri_plugin_opener::init())
    .plugin(tauri_plugin_notification::init())
    .plugin(tauri_plugin_process::init())
    .plugin(tauri_plugin_updater::Builder::new().build())
    .plugin(tauri_plugin_autostart::Builder::new()
      .args(["--minimized"])
      .build())
    .manage(commands::PendingUpdate(std::sync::Mutex::new(None)))
    .manage(app_state.clone())
    .setup(move |app| {
      let state = app_state.clone();

      // If not launched via autostart (--minimized), show the window
      let args: Vec<String> = std::env::args().collect();
      if !args.contains(&"--minimized".to_string()) {
        if let Some(window) = app.get_webview_window("main") {
          window.show().map_err(|e| e.to_string())?;
        }
      }

      // macOS: Hide Dock icon if config says so
      #[cfg(target_os = "macos")]
      {
        let hide_dock = {
          let state_guard = state.blocking_read();
          !state_guard.config_manager.get_config().general.show_dock_icon
        };
        if hide_dock {
          use cocoa::appkit::{NSApplication, NSApplicationActivationPolicy};
          use cocoa::base::nil;
          unsafe {
            let app = NSApplication::sharedApplication(nil);
            app.setActivationPolicy_(
              NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
            );
          }
        }
      }

      // Initialize state asynchronously
      tauri::async_runtime::spawn(async move {
        if let Err(e) = state.read().await.initialize().await {
          log::error!("Failed to initialize app state: {}", e);
        }
      });

      // Create and register NotificationManager
      let notification_manager = NotificationManager::new(
        app.handle().clone(),
        config_manager.clone(),
      );
      let notification_manager = Arc::new(RwLock::new(notification_manager));
      app.manage(notification_manager.clone());

      // Initialize NotificationManager asynchronously
      let init_notification_mgr = notification_manager.clone();
      tauri::async_runtime::spawn(async move {
        init_notification_mgr.write().await.initialize().await;
        log::info!("NotificationManager initialized");
      });

      // Start FileWatcher and connect to events
      let watcher_app_handle = app.handle().clone();
      tauri::async_runtime::spawn(async move {
        let mut watcher = FileWatcher::new();
        let projects_path = get_projects_base_path();

        // Create directory if it doesn't exist
        if !projects_path.exists() {
          if let Err(e) = tokio::fs::create_dir_all(&projects_path).await {
            log::error!("Failed to create projects directory: {}", e);
            return;
          }
        }

        // Start watching
        match watcher.watch(&projects_path).await {
          Ok(()) => {
            log::info!("FileWatcher started successfully");
            let mut receiver = watcher.receiver();

            // Process file change events
            while let Ok(event) = receiver.recv().await {
              events::emit_file_change(&watcher_app_handle, event);
            }
          }
          Err(e) => {
            log::error!("Failed to start FileWatcher: {}", e);
          }
        }
      });

      // Error detection pipeline — second FileWatcher for .jsonl changes
      let pipeline_handle = app.handle().clone();
      let pipeline_notification_mgr = notification_manager.clone();
      tauri::async_runtime::spawn(async move {
        let pipeline_config = ConfigManager::new();
        if let Err(e) = pipeline_config.initialize().await {
          log::error!("Failed to initialize error detection config: {}", e);
        }
        let detector = ErrorDetector::new(Arc::new(StdRwLock::new(pipeline_config)));

        let mut pipeline_watcher = FileWatcher::new();
        let projects_path = get_projects_base_path();

        if !projects_path.exists() {
          // Directory may not exist yet; skip silently (main watcher handles creation)
          return;
        }

        match pipeline_watcher.watch(&projects_path).await {
          Ok(()) => {
            log::info!("Error detection pipeline: watcher started");
            let mut receiver = pipeline_watcher.receiver();

            while let Ok(event) = receiver.recv().await {
              let path = Path::new(&event.path);

              // Only process .jsonl files, skip subagent files (handled by parent session)
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

              // Parse session file and run error detection
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

              // Feed detected errors to NotificationManager
              let mgr = pipeline_notification_mgr.read().await;
              for detected_error in errors {
                events::emit_error_detected(&pipeline_handle, &detected_error);
                // add_error internally emits notification:new and notification:updated
                let _ = mgr.add_error(detected_error).await;
              }
            }
          }
          Err(e) => {
            log::error!("Error detection pipeline: failed to start watcher: {}", e);
          }
        }
      });

      // Todo file watcher — watches ~/.claude/todos/ for checklist changes
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

      // Auto-check for updates after 3s (matching Electron behavior)
      let auto_update_handle = app.handle().clone();
      tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let updater = match auto_update_handle.updater() {
          Ok(u) => u,
          Err(_) => return, // Silently ignore — no endpoints configured
        };
        match updater.check().await {
          Ok(Some(update)) => {
            let version = update.version.clone();
            let _ = auto_update_handle.emit(
              "updater:status",
              &UpdaterStatus::Available { version },
            );
          }
          Ok(None) => {
            let _ =
              auto_update_handle.emit("updater:status", &UpdaterStatus::UpToDate);
          }
          Err(_) => {
            // Silently ignore auto-check errors — don't alarm the user
          }
        }
      });

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
