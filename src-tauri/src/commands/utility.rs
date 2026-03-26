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

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::parsing::claude_md_reader::{ClaudeMdReader, ClaudeMdFileInfo, ClaudeMdReadResult};

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
    Ok(())
}

/// Open a URL in the system browser.
#[tauri::command]
pub async fn open_external(url: String) -> Result<(), String> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("Invalid URL: must start with http:// or https://".to_string());
    }
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
    Ok(ZoomFactorResult { factor: 1.0 })
}

// =============================================================================
// CLAUDE.md Commands (synchronous methods)
// =============================================================================

/// Read all CLAUDE.md files for a project.
/// Note: ClaudeMdReader methods are synchronous.
#[tauri::command]
pub fn read_claude_md_files(project_root: String) -> ClaudeMdReadResult {
    let reader = ClaudeMdReader::new();
    reader.read_all_claude_md_files(&project_root)
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