//! Sessions 路由处理器。
//!
//! 对应 Tauri 命令：sessions.rs 中的会话相关命令。

use std::collections::HashSet;

use axum::{Json, extract::State, http::StatusCode};
use serde::Deserialize;

use crate::commands::AppState;
use crate::discovery::ProjectScanner;
use crate::http::state::HttpState;
use crate::parsing::parse_session_file;
use crate::types::domain::{PaginatedSessionsResult, Session, SessionMetrics};
use crate::types::chunks::SessionDetail;
use crate::utils::content_sanitizer::{
    extract_command_display, is_command_content, is_command_output_content,
    sanitize_display_content,
};
use crate::utils::{decode_path, extract_base_dir, get_projects_base_path};
use crate::analysis::ChunkBuilder;

use super::error_json;

/// 路径参数：project_id + session_id。
#[derive(Deserialize)]
pub struct ProjectSessionPath {
    pub project_id: String,
    pub session_id: String,
}

/// 获取指定项目下的所有会话列表。
///
/// GET /api/projects/{project_id}/sessions
pub async fn get_sessions(
    State(_state): State<HttpState>,
    axum::extract::Path(project_id): axum::extract::Path<String>,
) -> Result<Json<Vec<Session>>, (StatusCode, Json<super::ErrorResponse>)> {
    let base_path = get_projects_base_path();
    let project_dir_name = extract_base_dir(&project_id);
    let project_dir = base_path.join(&project_dir_name);

    if !project_dir.exists() {
        return Ok(Json(vec![]));
    }

    // Collect file entries with mtime for sorting
    struct FileEntry {
        path: std::path::PathBuf,
        session_id: String,
        mtime_ms: u64,
    }

    let mut file_entries = vec![];
    let mut entries = tokio::fs::read_dir(&project_dir)
        .await
        .map_err(|e| error_json(e.to_string()))?;

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
            let session_id = path
                .file_stem()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let mtime_ms = entry
                .metadata()
                .await
                .ok()
                .and_then(|m: std::fs::Metadata| m.modified().ok())
                .and_then(|t: std::time::SystemTime| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d: std::time::Duration| d.as_millis() as u64)
                .unwrap_or(0);
            file_entries.push(FileEntry {
                path,
                session_id,
                mtime_ms,
            });
        }
    }

    // Sort by file modification time (most recent first), matching Electron's mtimeMs sort.
    // Tie-breaker: session ID alphabetical ascending.
    file_entries.sort_by(|a, b| {
        if b.mtime_ms != a.mtime_ms {
            return b.mtime_ms.cmp(&a.mtime_ms);
        }
        a.session_id.cmp(&b.session_id)
    });

    let mut sessions = vec![];
    for file_entry in &file_entries {
        if let Some(session) = build_session_metadata(&file_entry.path, &project_id).await {
            sessions.push(session);
        }
    }

    Ok(Json(sessions))
}

/// 分页获取指定项目的会话列表。
///
/// GET /api/projects/{project_id}/sessions-paginated?cursor=&limit=
pub async fn get_sessions_paginated(
    State(_state): State<HttpState>,
    axum::extract::Path(project_id): axum::extract::Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<PaginatedSessionsResult>, (StatusCode, Json<super::ErrorResponse>)> {
    let cursor = params.get("cursor").cloned();
    let limit = params
        .get("limit")
        .and_then(|v| v.parse::<u32>().ok());

    let page_limit = limit.unwrap_or(50).min(100).max(1) as usize;

    let scanner = ProjectScanner::new();
    let all_sessions = scanner.list_sessions(&project_id);

    let total_count = all_sessions.len() as u32;

    // 定位游标位置
    let start_idx = if let Some(ref c) = cursor {
        all_sessions.iter().position(|s| &s.id == c).unwrap_or(0)
    } else {
        0
    };

    // 截取当前页数据
    let end_idx = (start_idx + page_limit).min(all_sessions.len());
    let sessions = all_sessions[start_idx..end_idx].to_vec();

    let has_more = end_idx < all_sessions.len();
    let next_cursor = if has_more {
        sessions.last().map(|s| s.id.clone())
    } else {
        None
    };

    Ok(Json(PaginatedSessionsResult {
        sessions,
        next_cursor,
        has_more,
        total_count,
    }))
}

/// 请求体：批量获取会话。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionsByIdsRequest {
    pub session_ids: Vec<String>,
    pub metadata_level: Option<String>,
}

/// 根据 ID 列表批量获取会话。
///
/// POST /api/projects/{project_id}/sessions-by-ids
pub async fn get_sessions_by_ids(
    State(_state): State<HttpState>,
    axum::extract::Path(project_id): axum::extract::Path<String>,
    Json(body): Json<SessionsByIdsRequest>,
) -> Result<Json<Vec<Session>>, (StatusCode, Json<super::ErrorResponse>)> {
    let id_set: HashSet<String> = body.session_ids.into_iter().collect();

    if id_set.is_empty() {
        return Ok(Json(Vec::new()));
    }

    let scanner = ProjectScanner::new();
    let all_sessions = scanner.list_sessions(&project_id);

    Ok(Json(
        all_sessions
            .into_iter()
            .filter(|s| id_set.contains(&s.id))
            .collect(),
    ))
}

/// 获取指定会话的完整详情（含可视化 Chunk 数据）。
///
/// GET /api/projects/{project_id}/sessions/{session_id}
pub async fn get_session_detail(
    State(state): State<HttpState>,
    axum::extract::Path(path): axum::extract::Path<ProjectSessionPath>,
) -> Result<Json<Option<SessionDetail>>, (StatusCode, Json<super::ErrorResponse>)> {
    let ProjectSessionPath {
        project_id,
        session_id,
    } = path;

    let cache_key = format!("{}/{}", project_id, session_id);

    // 优先查询缓存
    let app_state = state.app_state.read().await;
    if let Some(cached) = app_state.cache.get_session(&project_id, &session_id).await {
        if let Ok(detail) = serde_json::from_value(cached) {
            return Ok(Json(Some(detail)));
        }
    }
    drop(app_state);

    // 解析会话文件
    let base_path = get_projects_base_path();
    let project_dir = extract_base_dir(&project_id);
    let session_path = base_path
        .join(&project_dir)
        .join(format!("{}.jsonl", session_id));

    if !session_path.exists() {
        return Ok(Json(None));
    }

    let parsed = parse_session_file(&session_path).await;

    let fallback_path = extract_cwd_from_messages(&parsed)
        .unwrap_or_else(|| decode_path(&project_id));

    let session = build_session_metadata(&session_path, &project_id)
        .await
        .unwrap_or_else(|| Session {
            id: session_id.clone(),
            project_id: project_id.clone(),
            project_path: fallback_path,
            created_at: 0,
            todo_data: None,
            first_message: None,
            message_timestamp: None,
            has_subagents: !parsed.task_calls.is_empty(),
            message_count: parsed.messages.len() as u32,
            is_ongoing: Some(parsed.is_ongoing),
            git_branch: None,
            metadata_level: None,
            context_consumption: None,
            compaction_count: None,
            phase_breakdown: None,
        });

    // 使用 ChunkBuilder 将解析后的消息构建为可视化 Chunk
    let detail = ChunkBuilder::build_session_detail(
        session,
        parsed.messages.clone(),
        vec![],
    );

    // 缓存结果
    let app_state = state.app_state.read().await;
    app_state
        .cache
        .set_session(
            &project_id,
            &session_id,
            serde_json::to_value(&detail).unwrap_or_default(),
        )
        .await;

    Ok(Json(Some(detail)))
}

/// 获取会话的对话分组信息。
///
/// GET /api/projects/{project_id}/sessions/{session_id}/groups
pub async fn get_session_groups(
    State(_state): State<HttpState>,
    axum::extract::Path(path): axum::extract::Path<ProjectSessionPath>,
) -> Result<Json<Vec<crate::types::chunks::ConversationGroup>>, (StatusCode, Json<super::ErrorResponse>)>
{
    let ProjectSessionPath {
        project_id,
        session_id,
    } = path;

    let base_path = get_projects_base_path();
    let project_dir = extract_base_dir(&project_id);
    let session_path = base_path
        .join(&project_dir)
        .join(format!("{}.jsonl", session_id));

    if !session_path.exists() {
        return Ok(Json(vec![]));
    }

    let parsed = parse_session_file(&session_path).await;

    let subagents: Vec<crate::types::chunks::Process> = {
        let resolver =
            crate::discovery::subagent_resolver::SubagentResolver::new(get_projects_base_path(), std::sync::Arc::new(crate::infrastructure::fs_provider::LocalFsProvider::new()));
        resolver
            .resolve_subagents(&project_id, &session_id, Some(&parsed.task_calls), Some(&parsed.messages))
            .into_iter()
            .map(Into::into)
            .collect()
    };

    let groups =
        crate::analysis::conversation_group_builder::build_groups(&parsed.messages, &subagents);
    Ok(Json(groups))
}

/// 获取指定会话的指标数据（消息数、token 用量等）。
///
/// GET /api/projects/{project_id}/sessions/{session_id}/metrics
pub async fn get_session_metrics(
    State(_state): State<HttpState>,
    axum::extract::Path(path): axum::extract::Path<ProjectSessionPath>,
) -> Result<Json<Option<SessionMetrics>>, (StatusCode, Json<super::ErrorResponse>)> {
    let ProjectSessionPath {
        project_id,
        session_id,
    } = path;

    let base_path = get_projects_base_path();
    let project_dir = extract_base_dir(&project_id);
    let session_path = base_path
        .join(&project_dir)
        .join(format!("{}.jsonl", session_id));

    if !session_path.exists() {
        return Ok(Json(None));
    }

    let parsed = parse_session_file(&session_path).await;
    Ok(Json(Some(parsed.metrics)))
}

/// 获取会话的瀑布图数据（工具调用时序可视化）。
///
/// GET /api/projects/{project_id}/sessions/{session_id}/waterfall
pub async fn get_waterfall_data(
    State(_state): State<HttpState>,
    axum::extract::Path(path): axum::extract::Path<ProjectSessionPath>,
) -> Result<
    Json<Option<crate::analysis::waterfall_builder::WaterfallData>>,
    (StatusCode, Json<super::ErrorResponse>),
> {
    let ProjectSessionPath {
        project_id,
        session_id,
    } = path;

    let base_path = get_projects_base_path();
    let project_dir = extract_base_dir(&project_id);
    let session_path = base_path
        .join(&project_dir)
        .join(format!("{}.jsonl", session_id));

    if !session_path.exists() {
        return Ok(Json(None));
    }

    let parsed = parse_session_file(&session_path).await;

    let fallback_path = extract_cwd_from_messages(&parsed)
        .unwrap_or_else(|| decode_path(&project_id));

    let session = build_session_metadata(&session_path, &project_id)
        .await
        .unwrap_or_else(|| Session {
            id: session_id.clone(),
            project_id: project_id.clone(),
            project_path: fallback_path,
            created_at: 0,
            todo_data: None,
            first_message: None,
            message_timestamp: None,
            has_subagents: !parsed.task_calls.is_empty(),
            message_count: parsed.messages.len() as u32,
            is_ongoing: Some(parsed.is_ongoing),
            git_branch: None,
            metadata_level: None,
            context_consumption: None,
            compaction_count: None,
            phase_breakdown: None,
        });

    let subagents: Vec<crate::types::chunks::Process> = {
        let resolver =
            crate::discovery::subagent_resolver::SubagentResolver::new(get_projects_base_path(), std::sync::Arc::new(crate::infrastructure::fs_provider::LocalFsProvider::new()));
        resolver
            .resolve_subagents(&project_id, &session_id, Some(&parsed.task_calls), Some(&parsed.messages))
            .into_iter()
            .map(Into::into)
            .collect()
    };

    let detail = ChunkBuilder::build_session_detail(session, parsed.messages.clone(), subagents);
    let waterfall =
        crate::analysis::waterfall_builder::build_waterfall_data(&detail.chunks, &detail.processes);
    Ok(Json(Some(waterfall)))
}

// =============================================================================
// 辅助函数
// =============================================================================

/// 从 JSONL 文件构建会话元数据。
async fn build_session_metadata(path: &std::path::Path, project_id: &str) -> Option<Session> {
    let filename = path.file_stem()?.to_string_lossy().to_string();
    let metadata = path.metadata().ok()?;

    let parsed = parse_session_file(path).await;

    // Note: Electron does NOT check isMeta for title extraction — it processes all type='user' entries.
    // Slash commands are meta messages (isMeta: true) — we must still detect them as command fallback titles.
    let mut first_user_text: Option<String> = None;
    let mut first_command_text: Option<String> = None;
    let mut first_timestamp: Option<String> = None;

    for msg in &parsed.messages {
        if msg.message_type != crate::types::domain::MessageType::User {
            continue;
        }
        if first_timestamp.is_none() {
            first_timestamp = Some(msg.timestamp.clone());
        }

        let text = match &msg.content {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Array(arr) => arr
                .iter()
                .filter_map(|block| {
                    if block.get("type")?.as_str()? == "text" {
                        block.get("text")?.as_str().map(|s| s.trim().to_string())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
            _ => continue,
        };

        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }

        if is_command_output_content(trimmed)
            || trimmed.starts_with("[Request interrupted by user")
        {
            continue;
        }

        // Store command-name as fallback, keep looking for real text.
        // Match Electron's `content.startsWith('<command-name>')` check exactly.
        if trimmed.starts_with("<command-name>") {
            if first_command_text.is_none() {
                first_command_text = extract_command_display(trimmed);
            }
            continue;
        }

        // Real user text — Electron does NOT check isMeta here.
        let sanitized = sanitize_display_content(trimmed);
        if sanitized.is_empty() {
            continue;
        }
        first_user_text = Some(sanitized.chars().take(500).collect());
        break;
    }

    let first_message = first_user_text.or(first_command_text);

    let project_path = extract_cwd_from_messages(&parsed)
        .unwrap_or_else(|| decode_path(project_id));

    // createdAt: use first message timestamp from JSONL, fallback to file birth time.
    // This matches Electron's buildSessionMetadata() behavior for date grouping.
    let birthtime_ms = metadata
        .created()
        .map(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64
        })
        .unwrap_or(0);
    let created_at = first_timestamp
        .as_ref()
        .and_then(|ts| {
            chrono::DateTime::parse_from_rfc3339(ts)
                .or_else(|_| chrono::DateTime::parse_from_rfc2822(ts))
                .ok()
                .and_then(|dt| dt.timestamp_millis().try_into().ok())
        })
        .unwrap_or(birthtime_ms);

    Some(Session {
        id: filename,
        project_id: project_id.to_string(),
        project_path,
        created_at,
        todo_data: None,
        first_message,
        message_timestamp: first_timestamp,
        has_subagents: !parsed.task_calls.is_empty(),
        message_count: parsed.messages.len() as u32,
        is_ongoing: Some(parsed.is_ongoing),
        git_branch: parsed.messages.first().and_then(|m| m.git_branch.clone()),
        metadata_level: None,
        context_consumption: None,
        compaction_count: None,
        phase_breakdown: None,
    })
}

/// 提取第一条非空 cwd。
fn extract_cwd_from_messages(
    parsed: &crate::parsing::ParsedSession,
) -> Option<String> {
    for msg in &parsed.messages {
        if let Some(ref cwd) = msg.cwd {
            if !cwd.is_empty() {
                return Some(cwd.clone());
            }
        }
    }
    None
}
