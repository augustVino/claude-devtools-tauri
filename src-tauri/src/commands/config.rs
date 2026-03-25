use tauri::{command, State};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::commands::AppState;
use crate::types::config::AppConfig;

// =============================================================================
// Config Commands
// =============================================================================

#[command]
pub async fn get_config(
    state: State<'_, Arc<RwLock<AppState>>>,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.get_config())
}

#[command]
pub async fn update_config(
    state: State<'_, Arc<RwLock<AppState>>>,
    section: String,
    data: serde_json::Value,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    app_state.config_manager.update_config(&section, data)
}

// =============================================================================
// Notification Ignore Regex
// =============================================================================

#[command]
pub async fn add_ignore_regex(
    state: State<'_, Arc<RwLock<AppState>>>,
    pattern: String,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    app_state.config_manager.add_ignore_regex(pattern)
}

#[command]
pub async fn remove_ignore_regex(
    state: State<'_, Arc<RwLock<AppState>>>,
    pattern: String,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.remove_ignore_regex(pattern))
}

// =============================================================================
// Session Pin/Hide
// =============================================================================

#[command]
pub async fn pin_session(
    state: State<'_, Arc<RwLock<AppState>>>,
    project_id: String,
    session_id: String,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.pin_session(project_id, session_id))
}

#[command]
pub async fn unpin_session(
    state: State<'_, Arc<RwLock<AppState>>>,
    project_id: String,
    session_id: String,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.unpin_session(project_id, session_id))
}

#[command]
pub async fn hide_session(
    state: State<'_, Arc<RwLock<AppState>>>,
    project_id: String,
    session_id: String,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.hide_session(project_id, session_id))
}

#[command]
pub async fn unhide_session(
    state: State<'_, Arc<RwLock<AppState>>>,
    project_id: String,
    session_id: String,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.unhide_session(project_id, session_id))
}

// =============================================================================
// Snooze
// =============================================================================

#[command]
pub async fn snooze(
    state: State<'_, Arc<RwLock<AppState>>>,
    minutes: u32,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.snooze(minutes))
}

#[command]
pub async fn clear_snooze(
    state: State<'_, Arc<RwLock<AppState>>>,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.clear_snooze())
}
