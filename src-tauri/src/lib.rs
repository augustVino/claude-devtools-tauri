mod analysis;
mod commands;
mod constants;
mod discovery;
mod events;
mod infrastructure;
mod parsing;
mod types;
mod utils;

use std::sync::Arc;
use tokio::sync::RwLock;
use commands::AppState;
use infrastructure::FileWatcher;
use utils::get_projects_base_path;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  let app_state = Arc::new(RwLock::new(AppState::new()));

  tauri::Builder::default()
    .plugin(tauri_plugin_dialog::init())
    .plugin(tauri_plugin_opener::init())
    .manage(app_state.clone())
    .setup(move |app| {
      let app_handle = app.handle().clone();
      let state = app_state.clone();

      // Initialize state asynchronously
      tauri::async_runtime::spawn(async move {
        if let Err(e) = state.read().await.initialize().await {
          log::error!("Failed to initialize app state: {}", e);
        }
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
      commands::version::get_app_version,
      commands::sessions::get_sessions,
      commands::sessions::get_session_detail,
      commands::sessions::get_session_metrics,
      commands::sessions::get_sessions_paginated,
      commands::sessions::get_sessions_by_ids,
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
      commands::search::search_sessions,
      commands::search::search_all_projects,
      commands::validation::validate_path,
      commands::validation::validate_mentions,
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
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
