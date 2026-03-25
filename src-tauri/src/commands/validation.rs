//! IPC Handlers for Validation Operations.
//!
//! Handlers:
//! - validate_path: Validate a filesystem path
//! - validate_mentions: Validate file mentions

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Validation result for a path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathValidationResult {
    pub valid: bool,
    pub error: Option<String>,
    pub resolved_path: Option<String>,
}

/// Validate a filesystem path.
#[tauri::command]
pub async fn validate_path(path: String) -> Result<PathValidationResult, String> {
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
        return Ok(PathValidationResult {
            valid: false,
            error: Some(format!("Path does not exist: {}", expanded)),
            resolved_path: Some(expanded),
        });
    }

    Ok(PathValidationResult {
        valid: true,
        error: None,
        resolved_path: Some(expanded),
    })
}

/// Mentions validation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MentionsValidationResult {
    pub valid: bool,
    pub error: Option<String>,
}

/// Validate file mentions.
#[tauri::command]
pub async fn validate_mentions(mentions: Vec<String>) -> Result<MentionsValidationResult, String> {
    for mention in &mentions {
        if mention.contains("..") {
            return Ok(MentionsValidationResult {
                valid: false,
                error: Some(format!("Invalid path traversal in mention: {}", mention)),
            });
        }
    }

    Ok(MentionsValidationResult {
        valid: true,
        error: None,
    })
}