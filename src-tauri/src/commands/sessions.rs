use std::collections::HashSet;

use tauri::{command, State};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::discovery::ProjectScanner;
use crate::infrastructure::{DataCache, ConfigManager};
use crate::parsing::{parse_session_file, ParsedSession};
use crate::types::domain::{Session, SessionMetrics, PaginatedSessionsResult};
use crate::types::chunks::SessionDetail;
use crate::utils::content_sanitizer::{
    extract_command_display, sanitize_display_content, is_command_output_content, is_command_content,
};
use crate::utils::{decode_path, extract_base_dir, extract_project_name, get_projects_base_path};
use crate::analysis::ChunkBuilder;

/// Extract the first non-empty cwd from parsed session messages.
/// This avoids the lossy decode_path for project names containing dashes (e.g., "obsidian-stories").
fn extract_cwd_from_messages(parsed: &ParsedSession) -> Option<String> {
    for msg in &parsed.messages {
        if let Some(ref cwd) = msg.cwd {
            if !cwd.is_empty() {
                return Some(cwd.clone());
            }
        }
    }
    None
}

/// 跨命令共享的应用状态。
///
/// 包含数据缓存和配置管理器，通过 `Arc<RwLock<AppState>>` 注入到各 Tauri command 中。
pub struct AppState {
    pub cache: DataCache,
    pub config_manager: Arc<ConfigManager>,
}

impl AppState {
    /// 使用给定的配置管理器创建新的应用状态。
    pub fn new(config_manager: Arc<ConfigManager>) -> Self {
        Self {
            cache: DataCache::new(),
            config_manager,
        }
    }

    /// 初始化应用状态，包括异步加载配置文件。
    pub async fn initialize(&self) -> Result<(), String> {
        self.config_manager.initialize().await
    }
}

// =============================================================================
// 会话命令
// =============================================================================

/// 获取指定项目下的所有会话列表。
///
/// 扫描项目目录下的 `.jsonl` 文件，构建会话元数据，并按创建时间降序排列。
#[command]
pub async fn get_sessions(project_id: String) -> Result<Vec<Session>, String> {
    let base_path = get_projects_base_path();
    let project_dir_name = extract_base_dir(&project_id);
    let project_dir = base_path.join(&project_dir_name);

    if !project_dir.exists() {
        return Ok(vec![]);
    }

    let mut sessions = vec![];
    let mut entries = tokio::fs::read_dir(&project_dir)
        .await
        .map_err(|e| e.to_string())?;

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
            if let Some(session) = build_session_metadata(&path, &project_id).await {
                sessions.push(session);
            }
        }
    }

    // 按创建时间降序排列
    sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    Ok(sessions)
}

/// 获取指定会话的完整详情（含可视化 Chunk 数据）。
///
/// 优先从缓存读取，缓存未命中时解析 JSONL 文件并通过 `ChunkBuilder` 构建详情。
#[command]
pub async fn get_session_detail(
    state: State<'_, Arc<RwLock<AppState>>>,
    project_id: String,
    session_id: String,
) -> Result<Option<SessionDetail>, String> {
    let cache_key = format!("{}/{}", project_id, session_id);

    // 优先查询缓存
    let app_state = state.read().await;
    if let Some(cached) = app_state.cache.get_session(&project_id, &session_id).await {
        if let Ok(detail) = serde_json::from_value(cached) {
            return Ok(Some(detail));
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
        return Ok(None);
    }

    let parsed = parse_session_file(&session_path).await;

    // Prefer cwd from session file over lossy decode_path (handles dashes in project names)
    let fallback_path = extract_cwd_from_messages(&parsed)
        .unwrap_or_else(|| decode_path(&project_id));

    let session = build_session_metadata(&session_path, &project_id).await.unwrap_or_else(|| Session {
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
        vec![], // 子 Agent 数据在后续阶段填充
    );

    // 缓存结果
    let app_state = state.read().await;
    app_state.cache.set_session(&project_id, &session_id, serde_json::to_value(&detail).unwrap_or_default()).await;

    Ok(Some(detail))
}

/// 获取指定会话的指标数据（消息数、token 用量等）。
#[command]
pub async fn get_session_metrics(
    project_id: String,
    session_id: String,
) -> Result<Option<SessionMetrics>, String> {
    let base_path = get_projects_base_path();
    let project_dir = extract_base_dir(&project_id);
    let session_path = base_path
        .join(&project_dir)
        .join(format!("{}.jsonl", session_id));

    if !session_path.exists() {
        return Ok(None);
    }

    let parsed = parse_session_file(&session_path).await;
    Ok(Some(parsed.metrics))
}

// =============================================================================
// 项目命令
// =============================================================================

/// 获取所有项目列表。
///
/// 扫描 `~/.claude/projects/` 下的所有子目录，统计每个目录下的会话数量。
#[command]
pub async fn get_projects() -> Result<Vec<crate::types::domain::Project>, String> {
    let base_path = get_projects_base_path();

    if !base_path.exists() {
        return Ok(vec![]);
    }

    let mut projects = vec![];
    let mut entries = tokio::fs::read_dir(&base_path)
        .await
        .map_err(|e| e.to_string())?;

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.is_dir() {
            let project_id = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let project_path = decode_path(&project_id);
            let name = extract_project_name(&project_id, None);

            // 统计会话数量
            let session_count = count_sessions_in_dir(&path).await;

            projects.push(crate::types::domain::Project {
                id: project_id,
                path: project_path,
                name,
                sessions: vec![], // 按需加载
                created_at: path.metadata()
                    .and_then(|m| m.created())
                    .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64)
                    .unwrap_or(0),
                most_recent_session: None,
            });
        }
    }

    // 按创建时间降序排列（后续可改为按最近会话排序）
    projects.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    Ok(projects)
}

// =============================================================================
// 会话分页命令
// =============================================================================

/// 分页获取指定项目的会话列表。
///
/// 支持基于游标的分页，默认每页 50 条，最大 100 条。
#[command]
pub async fn get_sessions_paginated(
    project_id: String,
    cursor: Option<String>,
    limit: Option<u32>,
) -> Result<PaginatedSessionsResult, String> {
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

    Ok(PaginatedSessionsResult {
        sessions,
        next_cursor,
        has_more,
        total_count,
    })
}

/// 根据会话 ID 列表批量获取会话。
#[command]
pub async fn get_sessions_by_ids(
    project_id: String,
    session_ids: Vec<String>,
) -> Result<Vec<Session>, String> {
    let id_set: HashSet<String> = session_ids.into_iter().collect();

    if id_set.is_empty() {
        return Ok(Vec::new());
    }

    let scanner = ProjectScanner::new();
    let all_sessions = scanner.list_sessions(&project_id);

    Ok(all_sessions
        .into_iter()
        .filter(|s| id_set.contains(&s.id))
        .collect())
}

// =============================================================================
// 会话分组
// =============================================================================

/// 获取会话的对话分组信息。
///
/// 将会话消息按照对话结构分组，同时解析子 Agent 进程数据。
#[command]
pub async fn get_session_groups(
    project_id: String,
    session_id: String,
) -> Result<Vec<crate::types::chunks::ConversationGroup>, String> {
    let base_path = get_projects_base_path();
    let project_dir = extract_base_dir(&project_id);
    let session_path = base_path
        .join(&project_dir)
        .join(format!("{}.jsonl", session_id));

    if !session_path.exists() {
        return Ok(vec![]);
    }

    let parsed = parse_session_file(&session_path).await;

    let subagents: Vec<crate::types::chunks::Process> = {
        let resolver = crate::discovery::subagent_resolver::SubagentResolver::new(
            get_projects_base_path(),
        );
        resolver
            .resolve_subagents(&project_id, &session_id)
            .into_iter()
            .map(Into::into)
            .collect()
    };

    let groups =
        crate::analysis::conversation_group_builder::build_groups(&parsed.messages, &subagents);
    Ok(groups)
}

// =============================================================================
// 瀑布图数据
// =============================================================================

/// 获取会话的瀑布图数据（工具调用时序可视化）。
///
/// 解析会话消息和子 Agent 数据后，构建工具调用的时序瀑布图。
#[command]
pub async fn get_waterfall_data(
    project_id: String,
    session_id: String,
) -> Result<Option<crate::analysis::waterfall_builder::WaterfallData>, String> {
    let base_path = get_projects_base_path();
    let project_dir = extract_base_dir(&project_id);
    let session_path = base_path
        .join(&project_dir)
        .join(format!("{}.jsonl", session_id));

    if !session_path.exists() {
        return Ok(None);
    }

    let parsed = parse_session_file(&session_path).await;

    // Prefer cwd from session file over lossy decode_path (handles dashes in project names)
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
        let resolver = crate::discovery::subagent_resolver::SubagentResolver::new(
            get_projects_base_path(),
        );
        resolver
            .resolve_subagents(&project_id, &session_id)
            .into_iter()
            .map(Into::into)
            .collect()
    };

    let detail = ChunkBuilder::build_session_detail(session, parsed.messages.clone(), subagents);
    let waterfall =
        crate::analysis::waterfall_builder::build_waterfall_data(&detail.chunks, &detail.processes);
    Ok(Some(waterfall))
}

// =============================================================================
// 辅助函数
// =============================================================================

/// 从 JSONL 文件构建会话元数据。
///
/// 解析文件获取首条用户消息作为标题，同时提取创建时间、消息数量、
/// 子 Agent 标记、Git 分支等元信息。
async fn build_session_metadata(path: &std::path::Path, project_id: &str) -> Option<Session> {
    let filename = path.file_stem()?.to_string_lossy().to_string();
    let metadata = path.metadata().ok()?;

    let parsed = parse_session_file(path).await;

    // 提取首条用户消息作为标题（与 Electron 版 analyzeSessionFileMetadata 对齐）
    let mut first_user_text: Option<String> = None;
    let mut first_command_text: Option<String> = None;

    for msg in &parsed.messages {
        if msg.message_type != crate::types::domain::MessageType::User {
            continue;
        }
        if msg.is_meta {
            continue;
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

        // 跳过命令输出和用户中断消息
        if is_command_output_content(trimmed)
            || trimmed.starts_with("[Request interrupted by user")
        {
            continue;
        }

        // 保存命令名作为备选标题，继续查找真实用户文本
        if is_command_content(trimmed) {
            if first_command_text.is_none() {
                first_command_text = extract_command_display(trimmed);
            }
            continue;
        }

        // 找到真实用户文本 — 清理并截断到 500 字符
        let sanitized = sanitize_display_content(trimmed);
        if sanitized.is_empty() {
            continue;
        }
        first_user_text = Some(sanitized.chars().take(500).collect());
        break;
    }

    let first_message = first_user_text.or(first_command_text);

    // Prefer cwd from session file over lossy decode_path (handles dashes in project names)
    let project_path = extract_cwd_from_messages(&parsed)
        .unwrap_or_else(|| decode_path(project_id));

    Some(Session {
        id: filename,
        project_id: project_id.to_string(),
        project_path,
        created_at: metadata.created()
            .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64)
            .unwrap_or(0),
        todo_data: None,
        first_message,
        message_timestamp: None,
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

/// 统计目录下的 `.jsonl` 会话文件数量。
async fn count_sessions_in_dir(dir: &std::path::Path) -> u32 {
    let mut count = 0u32;
    if let Ok(mut entries) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.path().extension().map(|e| e == "jsonl").unwrap_or(false) {
                count += 1;
            }
        }
    }
    count
}
