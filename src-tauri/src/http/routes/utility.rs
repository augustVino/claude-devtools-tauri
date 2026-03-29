//! Utility 路由处理器。
//!
//! 对应 Tauri 命令：utility.rs 中的实用命令。
//! 注意：read_claude_md_files 和 read_agent_configs 是同步文件 I/O 函数，
//! 使用 tokio::task::spawn_blocking 包装以避免阻塞异步运行时。

use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::http::state::HttpState;
use crate::parsing::claude_md_reader::{ClaudeMdReader, ClaudeMdFileInfo};

use super::error_json;

/// 获取应用版本号。
///
/// GET /api/version
/// 注意：httpClient.ts 的 getAppVersion 期望返回裸字符串，不是 JSON 对象。
pub async fn get_version(
    State(_state): State<HttpState>,
) -> Json<String> {
    Json(env!("CARGO_PKG_VERSION").to_string())
}

/// 请求体：读取 CLAUDE.md 文件。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadClaudeMdRequest {
    pub project_root: String,
}

/// 读取项目所有 CLAUDE.md 文件。
///
/// POST /api/read-claude-md
pub async fn read_claude_md(
    State(_state): State<HttpState>,
    Json(body): Json<ReadClaudeMdRequest>,
) -> Result<
    Json<std::collections::HashMap<String, ClaudeMdFileInfo>>,
    (StatusCode, Json<super::ErrorResponse>),
> {
    let project_root = body.project_root;
    let result = tokio::task::spawn_blocking(move || {
        let reader = ClaudeMdReader::new();
        reader.read_all_claude_md_files(&project_root).files
    })
    .await
    .map_err(|e| error_json(format!("Task join error: {}", e)))?;

    Ok(Json(result))
}

/// 请求体：读取目录级 CLAUDE.md。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadDirectoryClaudeMdRequest {
    pub dir_path: String,
}

/// 读取指定目录的 CLAUDE.md 文件。
///
/// POST /api/read-directory-claude-md
pub async fn read_directory_claude_md(
    State(_state): State<HttpState>,
    Json(body): Json<ReadDirectoryClaudeMdRequest>,
) -> Result<Json<ClaudeMdFileInfo>, (StatusCode, Json<super::ErrorResponse>)> {
    let directory = body.dir_path;
    let result = tokio::task::spawn_blocking(move || {
        let reader = ClaudeMdReader::new();
        reader.read_directory_claude_md(&directory)
    })
    .await
    .map_err(|e| error_json(format!("Task join error: {}", e)))?;

    Ok(Json(result))
}

/// 请求体：读取 mentioned file。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadMentionedFileRequest {
    pub absolute_path: String,
    pub project_root: String,
    pub max_tokens: Option<usize>,
}

/// mentioned file 信息（上下文注入）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionedFileInfo {
    pub path: String,
    pub exists: bool,
    pub char_count: usize,
    pub estimated_tokens: usize,
}

/// 读取 mentioned file（用于上下文注入）。
///
/// POST /api/read-mentioned-file
pub async fn read_mentioned_file(
    State(_state): State<HttpState>,
    Json(body): Json<ReadMentionedFileRequest>,
) -> Result<Json<Option<MentionedFileInfo>>, (StatusCode, Json<super::ErrorResponse>)> {
    let max_tokens_limit = body.max_tokens.unwrap_or(25000);
    let path = Path::new(&body.absolute_path);

    // 跳过不存在的路径和目录
    if !path.exists() || path.is_dir() {
        return Ok(Json(None));
    }

    match tokio::fs::read_to_string(path).await {
        Ok(content) => {
            if content.len() > 1_000_000 {
                return Ok(Json(None));
            }

            let char_count = content.len();
            // 简单 token 估算：~4 字符/token
            let estimated_tokens = char_count / 4;

            if estimated_tokens > max_tokens_limit {
                return Ok(Json(None));
            }

            Ok(Json(Some(MentionedFileInfo {
                path: body.absolute_path,
                exists: true,
                char_count,
                estimated_tokens,
            })))
        }
        Err(_) => Ok(Json(None)),
    }
}

/// Agent 配置。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    pub name: String,
    pub color: Option<String>,
}

/// 请求体：读取 agent 配置。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadAgentConfigsRequest {
    pub project_root: String,
}

/// 读取 .claude/agents/ 目录中的 agent 配置。
///
/// POST /api/read-agent-configs
pub async fn read_agent_configs(
    State(_state): State<HttpState>,
    Json(body): Json<ReadAgentConfigsRequest>,
) -> Result<
    Json<std::collections::HashMap<String, AgentConfig>>,
    (StatusCode, Json<super::ErrorResponse>),
> {
    let project_root = body.project_root;
    let result = tokio::task::spawn_blocking(move || {
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
    })
    .await
    .map_err(|e| error_json(format!("Task join error: {}", e)))?;

    Ok(Json(result))
}

/// No-op handler for routes that require native UI interaction (open-path, open-external).
pub async fn no_op() -> (StatusCode, Json<super::SuccessResponse>) {
    super::success_response()
}
