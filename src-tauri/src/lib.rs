mod analysis;
mod commands;
mod constants;
mod discovery;
mod infrastructure;
mod parsing;
mod types;
mod utils;

use std::sync::Arc;
use tokio::sync::RwLock;
use commands::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  let app_state = Arc::new(RwLock::new(AppState::new()));

  tauri::Builder::default()
    .plugin(tauri_plugin_dialog::init())
    .plugin(tauri_plugin_opener::init())
    .manage(app_state.clone())
    .setup(move |app| {
      // Initialize state asynchronously
      let state = app_state.clone();
      tauri::async_runtime::spawn(async move {
        if let Err(e) = state.read().await.initialize().await {
          log::error!("Failed to initialize app state: {}", e);
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
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
