use tauri::{command, State};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::commands::AppState;
use crate::types::config::AppConfig;
use tauri_plugin_opener::OpenerExt;

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

// =============================================================================
// Notification Triggers
// =============================================================================

#[command]
pub async fn add_trigger(
    state: State<'_, Arc<RwLock<AppState>>>,
    trigger: crate::types::config::NotificationTrigger,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    app_state.config_manager.add_trigger(trigger)
}

#[command]
pub async fn update_trigger(
    state: State<'_, Arc<RwLock<AppState>>>,
    trigger_id: String,
    updates: serde_json::Value,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    app_state.config_manager.update_trigger(&trigger_id, updates)
}

#[command]
pub async fn remove_trigger(
    state: State<'_, Arc<RwLock<AppState>>>,
    trigger_id: String,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    app_state.config_manager.remove_trigger(&trigger_id)
}

#[command]
pub async fn get_triggers(
    state: State<'_, Arc<RwLock<AppState>>>,
) -> Result<Vec<crate::types::config::NotificationTrigger>, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.get_triggers())
}

#[command]
pub async fn test_trigger(
    trigger: crate::types::config::NotificationTrigger,
) -> Result<crate::types::config::TriggerTestResult, String> {
    use crate::discovery::project_scanner::ProjectScanner;
    use crate::error::error_trigger_tester;

    let scanner = ProjectScanner::new();
    Ok(error_trigger_tester::test_trigger(&trigger, &scanner, None).await)
}

// =============================================================================
// Repository Ignore
// =============================================================================

#[command]
pub async fn add_ignore_repository(
    state: State<'_, Arc<RwLock<AppState>>>,
    repository_id: String,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.add_ignore_repository(repository_id))
}

#[command]
pub async fn remove_ignore_repository(
    state: State<'_, Arc<RwLock<AppState>>>,
    repository_id: String,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.remove_ignore_repository(repository_id))
}

// =============================================================================
// Bulk Session Hide/Unhide
// =============================================================================

#[command]
pub async fn hide_sessions(
    state: State<'_, Arc<RwLock<AppState>>>,
    project_id: String,
    session_ids: Vec<String>,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.hide_sessions(project_id, session_ids))
}

#[command]
pub async fn unhide_sessions(
    state: State<'_, Arc<RwLock<AppState>>>,
    project_id: String,
    session_ids: Vec<String>,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.unhide_sessions(project_id, session_ids))
}

// =============================================================================
// Editor & Claude Root
// =============================================================================

#[command]
pub async fn open_in_editor(
    app: tauri::AppHandle,
    state: State<'_, Arc<RwLock<AppState>>>,
) -> Result<(), String> {
    let app_state = state.read().await;
    let config_path = app_state.config_manager.get_config_path();
    drop(app_state);

    let config_path_str = config_path.to_string_lossy().to_string();

    // Try $VISUAL
    if let Ok(editor) = std::env::var("VISUAL") {
        if try_spawn_editor(&editor, &config_path_str).await {
            return Ok(());
        }
    }

    // Try $EDITOR
    if let Ok(editor) = std::env::var("EDITOR") {
        if try_spawn_editor(&editor, &config_path_str).await {
            return Ok(());
        }
    }

    // Try common editors
    for editor in &["cursor", "code", "zed", "subl"] {
        if try_spawn_editor(editor, &config_path_str).await {
            return Ok(());
        }
    }

    // Fallback: system default
    app.opener()
        .open_path(&config_path_str, None::<&str>)
        .map_err(|e| format!("Failed to open config file: {}", e))
}

#[command]
pub async fn get_claude_root_info(
    state: State<'_, Arc<RwLock<AppState>>>,
) -> Result<crate::types::config::ClaudeRootInfo, String> {
    let app_state = state.read().await;
    let config = app_state.config_manager.get_config();
    drop(app_state);

    let custom_path = config.general.claude_root_path.clone();
    let default_path = dirs::home_dir()
        .map(|h| h.join(".claude").to_string_lossy().to_string())
        .unwrap_or_default();
    let resolved_path = custom_path.clone().unwrap_or_else(|| default_path.clone());

    Ok(crate::types::config::ClaudeRootInfo {
        default_path,
        resolved_path,
        custom_path,
    })
}

// =============================================================================
// Projects Directory Check
// =============================================================================

#[command]
pub async fn check_projects_dir_exists(path: String) -> Result<bool, String> {
    Ok(std::path::Path::new(&path).join("projects").is_dir())
}

/// Try to spawn an editor process. Returns true if successful.
async fn try_spawn_editor(editor: &str, file_path: &str) -> bool {
    tokio::process::Command::new(editor)
        .arg(file_path)
        .spawn()
        .map(|_| true)
        .unwrap_or(false)
}
