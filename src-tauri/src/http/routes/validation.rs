//! Validation 路由处理器。
//!
//! 对应 Tauri 命令：validation.rs 中的验证命令。

use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::http::path_validation::is_path_contained;
use crate::http::state::HttpState;

use super::error_json;

/// 路径验证结果（匹配 httpClient.ts 的返回类型）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathValidationResult {
    pub exists: bool,
    pub is_directory: Option<bool>,
}

/// 请求体：路径验证（匹配 httpClient.ts 发送的 {relativePath, projectPath}）。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidatePathRequest {
    pub relative_path: String,
    pub project_path: String,
}

/// 验证文件系统路径。
///
/// POST /api/validate/path
pub async fn validate_path(
    State(state): State<HttpState>,
    Json(body): Json<ValidatePathRequest>,
) -> Result<Json<PathValidationResult>, (StatusCode, Json<super::ErrorResponse>)> {
    // 组合 projectPath + relativePath 得到完整路径
    let full_path = Path::new(&body.project_path).join(&body.relative_path);

    // 处理 ~ 展开
    let expanded = if full_path.starts_with("~") {
        if let Some(home) = dirs::home_dir() {
            let full_str = full_path.to_string_lossy().to_string();
            let remainder = full_str[1..].trim_start_matches('/');
            home.join(remainder).to_path_buf()
        } else {
            full_path
        }
    } else {
        full_path
    };

    // 路径遍历防护
    if !is_path_contained(&expanded, Path::new(&body.project_path)) {
        log::warn!(
            "validate-path blocked path traversal: {}",
            body.relative_path
        );
        return Ok(Json(PathValidationResult {
            exists: false,
            is_directory: None,
        }));
    }

    // SSH-aware: 通过 fs_provider 检查存在性和类型
    let fs_provider = {
        let mgr = state.context_manager.read().await;
        let active = mgr
            .get_active()
            .ok_or_else(|| error_json("No active ServiceContext".to_string()))?;
        let ctx = active.read().await;
        ctx.fs_provider.clone()
    };

    let stat = match fs_provider.stat(&expanded) {
        Ok(s) => s,
        Err(_) => {
            return Ok(Json(PathValidationResult {
                exists: false,
                is_directory: None,
            }))
        }
    };

    Ok(Json(PathValidationResult {
        exists: true,
        is_directory: Some(stat.is_directory),
    }))
}

/// Mention 条目（匹配 httpClient.ts 发送的 {type, value} 结构）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionEntry {
    #[serde(rename = "type")]
    pub mention_type: String,
    pub value: String,
}

/// 请求体：mentions 验证。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidateMentionsRequest {
    pub mentions: Vec<MentionEntry>,
    pub project_path: String,
}

/// 验证文件 mentions（返回 Record<string, boolean>）。
///
/// POST /api/validate/mentions
pub async fn validate_mentions(
    State(state): State<HttpState>,
    Json(body): Json<ValidateMentionsRequest>,
) -> Result<Json<std::collections::HashMap<String, bool>>, (StatusCode, Json<super::ErrorResponse>)>
{
    let base = Path::new(&body.project_path);
    let fs_provider = {
        let mgr = state.context_manager.read().await;
        let active = mgr
            .get_active()
            .ok_or_else(|| error_json("No active ServiceContext".to_string()))?;
        let ctx = active.read().await;
        ctx.fs_provider.clone()
    };
    let mut results = std::collections::HashMap::new();

    for mention in &body.mentions {
        if mention.mention_type != "path" {
            results.insert(format!("@{}", mention.value), false);
            continue;
        }

        let full_path = base.join(&mention.value);

        // 替换 contains("..") 为 is_path_contained
        if !is_path_contained(&full_path, base) {
            results.insert(format!("@{}", mention.value), false);
            continue;
        }

        // SSH-aware: fs_provider.exists 替代本地 full_path.exists()
        let exists = fs_provider.exists(&full_path).unwrap_or(false);
        results.insert(format!("@{}", mention.value), exists);
    }

    Ok(Json(results))
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
