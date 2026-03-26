//! IPC Handlers for Validation Operations.
//!
//! Handlers:
//! - validate_path: Validate a filesystem path
//! - validate_mentions: Validate file mentions
//! - scroll_to_line: Validate session scroll request

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

// =============================================================================
// Session Scroll
// =============================================================================

/// Result of a scroll-to-line request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrollToLineResult {
    pub success: bool,
    pub session_id: String,
    pub line_number: i64,
}

/// Validate a session scroll request and return success indicator.
/// The actual scrolling is handled by the frontend.
#[tauri::command]
pub async fn scroll_to_line(
    session_id: String,
    line_number: i64,
) -> Result<ScrollToLineResult, String> {
    if session_id.is_empty() || line_number < 0 {
        return Ok(ScrollToLineResult {
            success: false,
            session_id: String::new(),
            line_number: 0,
        });
    }

    Ok(ScrollToLineResult {
        success: true,
        session_id,
        line_number,
    })
}