//! IPC Handlers for Utility Operations.
//!
//! Handlers:
//! - open_path: Open a path in the system file manager
//! - open_external: Open a URL in the system browser
//! - get_zoom_factor: Get the current zoom factor
//! - read_claude_md_files: Read all CLAUDE.md files for a project
//! - read_directory_claude_md: Read a specific directory's CLAUDE.md file
//! - read_mentioned_file: Read a mentioned file for context injection
//! - read_agent_configs: Read agent configurations from .claude/agents/
//! - write_text_file: Write text content to a file

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;

use crate::parsing::claude_md_reader::{ClaudeMdReader, ClaudeMdFileInfo};

/// Open a path in the system file manager.
#[tauri::command]
pub async fn open_path(app: tauri::AppHandle, path: String) -> Result<(), String> {
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

    app.opener()
        .open_path(&expanded, None::<&str>)
        .map_err(|e| format!("Failed to open path: {}", e))?;
    Ok(())
}

/// Open a URL in the system browser.
#[tauri::command]
pub async fn open_external(app: tauri::AppHandle, url: String) -> Result<(), String> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("Invalid URL: must start with http:// or https://".to_string());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Zoom factor result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoomFactorResult {
    pub factor: f64,
}

/// Get the current zoom factor from app state.
/// Tauri v2 has set_zoom() but no zoom() getter, so we track it ourselves.
#[tauri::command]
pub async fn get_zoom_factor(
    zoom_state: State<'_, Arc<AtomicU64>>,
) -> Result<ZoomFactorResult, String> {
    let bits = zoom_state.load(Ordering::Relaxed);
    Ok(ZoomFactorResult {
        factor: f64::from_bits(bits),
    })
}

/// Set the zoom factor and persist it in app state.
/// Clamps to [0.5, 3.0] range (~50% to ~300%).
#[tauri::command]
pub async fn set_zoom_factor(
    app: AppHandle,
    zoom_state: State<'_, Arc<AtomicU64>>,
    factor: f64,
) -> Result<(), String> {
    let clamped = factor.clamp(0.5, 3.0);
    if let Some(window) = app.get_webview_window("main") {
        window
            .set_zoom(clamped)
            .map_err(|e| e.to_string())?;
    }
    zoom_state.store(clamped.to_bits(), Ordering::Relaxed);
    Ok(())
}

// =============================================================================
// CLAUDE.md Commands (synchronous methods)
// =============================================================================

/// Read all CLAUDE.md files for a project.
/// Note: ClaudeMdReader methods are synchronous.
/// Returns flat HashMap to match Electron IPC (which unwraps ClaudeMdReadResult.files).
#[tauri::command]
pub fn read_claude_md_files(
    project_root: String,
) -> std::collections::HashMap<String, ClaudeMdFileInfo> {
    let reader = ClaudeMdReader::new();
    reader.read_all_claude_md_files(&project_root).files
}

/// Read a specific directory's CLAUDE.md file.
#[tauri::command]
pub fn read_directory_claude_md(directory: String) -> ClaudeMdFileInfo {
    let reader = ClaudeMdReader::new();
    reader.read_directory_claude_md(&directory)
}

/// Mentioned file info for context injection (matches Electron MentionedFileInfo).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionedFileInfo {
    pub path: String,
    pub exists: bool,
    pub char_count: usize,
    pub estimated_tokens: usize,
}

/// Read a mentioned file for context injection.
/// Returns MentionedFileInfo with token count (matches Electron HTTP API behavior).
#[tauri::command]
pub async fn read_mentioned_file(
    file_path: String,
    _project_root: String,
    max_tokens: Option<usize>,
) -> Result<Option<MentionedFileInfo>, String> {
    let max_tokens_limit = max_tokens.unwrap_or(25000);
    let path = Path::new(&file_path);

    // Skip non-existent paths and directories
    if !path.exists() || path.is_dir() {
        return Ok(None);
    }

    match tokio::fs::read_to_string(path).await {
        Ok(content) => {
            if content.len() > 1_000_000 {
                return Ok(None);
            }

            let char_count = content.len();
            // Simple token estimation: ~4 chars per token
            let estimated_tokens = char_count / 4;

            if estimated_tokens > max_tokens_limit {
                return Ok(None);
            }

            Ok(Some(MentionedFileInfo {
                path: file_path,
                exists: true,
                char_count,
                estimated_tokens,
            }))
        }
        Err(_) => Ok(None),
    }
}

/// Agent config for IPC (matches frontend AgentConfig interface).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    pub name: String,
    pub color: Option<String>,
}

/// Read agent configurations from .claude/agents/ directory.
/// Returns a map from agent name to AgentConfig (matches Electron HTTP API behavior).
#[tauri::command]
pub fn read_agent_configs(project_root: String) -> std::collections::HashMap<String, AgentConfig> {
    let configs = crate::parsing::agent_config_reader::read_agent_configs(&project_root);
    configs
        .into_iter()
        .map(|(name, config)| {
            (
                name.clone(),
                AgentConfig {
                    name,
                    color: config.color,
                },
            )
        })
        .collect()
}

/// Write text content to a file at the given path.
/// Used by the export flow after the user picks a save location via the native dialog.
#[tauri::command]
pub async fn write_text_file(path: String, content: String) -> Result<(), String> {
    tokio::fs::write(&path, content)
        .await
        .map_err(|e| format!("Failed to write file: {}", e))
}

#[cfg(test)]
mod write_text_file_tests {
    use super::*;

    #[tokio::test]
    async fn test_write_text_file_creates_file() {
        let dir = tempfile::TempDir::new().expect("temp dir creation");
        let path = dir.path().join("test-export.md");
        let content = "# Hello\nThis is a test.".to_string();

        write_text_file(path.to_string_lossy().to_string(), content.clone())
            .await
            .expect("write should succeed");

        let read_back = tokio::fs::read_to_string(&path).await.expect("should read back");
        assert_eq!(read_back, content);
    }

    #[tokio::test]
    async fn test_write_text_file_overwrites_existing() {
        let dir = tempfile::TempDir::new().expect("temp dir creation");
        let path = dir.path().join("test-overwrite.md");

        write_text_file(path.to_string_lossy().to_string(), "old content".to_string())
            .await
            .expect("first write should succeed");

        write_text_file(path.to_string_lossy().to_string(), "new content".to_string())
            .await
            .expect("overwrite should succeed");

        let read_back = tokio::fs::read_to_string(&path).await.expect("should read back");
        assert_eq!(read_back, "new content");
    }

    #[tokio::test]
    async fn test_write_text_file_nonexistent_directory() {
        let result = write_text_file(
            "/nonexistent/tauri_test_dir/file.txt".to_string(),
            "content".to_string(),
        )
        .await;

        assert!(result.is_err(), "expected error for nonexistent directory");
        let err_msg = result.unwrap_err();
        assert!(
            err_msg.contains("Failed to write file"),
            "error message should contain 'Failed to write file', got: {}",
            err_msg
        );
    }
}