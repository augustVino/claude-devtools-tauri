//! Validation 路由处理器。
//!
//! 对应 Tauri 命令：validation.rs 中的验证命令。

use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::http::state::HttpState;

/// 路径验证结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathValidationResult {
    pub valid: bool,
    pub error: Option<String>,
    pub resolved_path: Option<String>,
}

/// 请求体：路径验证。
#[derive(Deserialize)]
pub struct ValidatePathRequest {
    pub path: String,
}

/// 验证文件系统路径。
///
/// POST /api/validate/path
pub async fn validate_path(
    State(_state): State<HttpState>,
    Json(body): Json<ValidatePathRequest>,
) -> Result<Json<PathValidationResult>, (StatusCode, Json<super::ErrorResponse>)> {
    let expanded = if body.path.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            let remainder = body.path[1..].trim_start_matches('/');
            home.join(remainder).to_string_lossy().to_string()
        } else {
            body.path.clone()
        }
    } else {
        body.path.clone()
    };

    let p = Path::new(&expanded);

    if !p.exists() {
        return Ok(Json(PathValidationResult {
            valid: false,
            error: Some(format!("Path does not exist: {}", expanded)),
            resolved_path: Some(expanded),
        }));
    }

    Ok(Json(PathValidationResult {
        valid: true,
        error: None,
        resolved_path: Some(expanded),
    }))
}

/// 请求体：mentions 验证。
#[derive(Deserialize)]
pub struct ValidateMentionsRequest {
    pub mentions: Vec<String>,
}

/// mentions 验证结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MentionsValidationResult {
    pub valid: bool,
    pub error: Option<String>,
}

/// 验证文件 mentions。
///
/// POST /api/validate/mentions
pub async fn validate_mentions(
    State(_state): State<HttpState>,
    Json(body): Json<ValidateMentionsRequest>,
) -> Result<Json<MentionsValidationResult>, (StatusCode, Json<super::ErrorResponse>)> {
    for mention in &body.mentions {
        if mention.contains("..") {
            return Ok(Json(MentionsValidationResult {
                valid: false,
                error: Some(format!("Invalid path traversal in mention: {}", mention)),
            }));
        }
    }

    Ok(Json(MentionsValidationResult {
        valid: true,
        error: None,
    }))
}

/// 请求体：滚动到行。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrollToLineRequest {
    pub session_id: String,
    pub line_number: i64,
}

/// 滚动到行结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrollToLineResult {
    pub success: bool,
    pub session_id: String,
    pub line_number: i64,
}

/// 验证会话滚动请求并返回成功指示。
///
/// POST /api/session/scroll-to-line
pub async fn scroll_to_line(
    State(_state): State<HttpState>,
    Json(body): Json<ScrollToLineRequest>,
) -> Result<Json<ScrollToLineResult>, (StatusCode, Json<super::ErrorResponse>)> {
    if body.session_id.is_empty() || body.line_number < 0 {
        return Ok(Json(ScrollToLineResult {
            success: false,
            session_id: String::new(),
            line_number: 0,
        }));
    }

    Ok(Json(ScrollToLineResult {
        success: true,
        session_id: body.session_id,
        line_number: body.line_number,
    }))
}
