//! IPC Handlers for Utility Operations.
//!
//! Handlers:
//! - open_path: Open a path in the system file manager
//! - open_external: Open a URL in the system browser
//! - get_zoom_factor: Get the current zoom factor
//! - on_zoom_factor_changed: Listen for zoom factor changes

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Open a path in the system file manager.
#[tauri::command]
pub async fn open_path(path: String) -> Result<(), String> {
    let expanded = if path.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            let remainder = path[1..].trim_start_matches('/');
            home.join(remainder).to_string_lossy().to_string()
        } else {
            path.clone()
        }
    } else {
        path.clone()
    };

    let p = Path::new(&expanded);
    if !p.exists() {
        return Err(format!("Path does not exist: {}", expanded));
    }

    // Use tauri-plugin-opener to open the path
    // For now, just return success
    Ok(())
}

/// Open a URL in the system browser.
#[tauri::command]
pub async fn open_external(url: String) -> Result<(), String> {
    // Validate URL
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("Invalid URL: must start with http:// or https://".to_string());
    }

    // Use tauri-plugin-opener to open the URL
    // For now, just return success
    Ok(())
}

/// Zoom factor result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoomFactorResult {
    pub factor: f64,
}

/// Get the current zoom factor.
#[tauri::command]
pub async fn get_zoom_factor() -> Result<ZoomFactorResult, String> {
    // Default zoom factor
    Ok(ZoomFactorResult { factor: 1.0 })
}

/// File change event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChangeEvent {
    pub event_type: String,
    pub path: String,
    pub project_id: Option<String>,
    pub session_id: Option<String>,
    pub is_subagent: bool,
}

/// Todo change event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoChangeEvent {
    pub session_id: String,
}