use std::collections::HashSet;

use tauri::{command, State};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::discovery::ProjectScanner;
use crate::infrastructure::{DataCache, ConfigManager};
use crate::parsing::{parse_session_file, ParsedSession};
use crate::types::domain::{Session, SessionMetrics, PaginatedSessionsResult, SessionsPaginationOptions};
use crate::types::chunks::SessionDetail;
use crate::utils::content_sanitizer::{
    extract_command_display, sanitize_display_content, is_command_output_content, is_command_content,
};
use crate::utils::{decode_path, extract_base_dir, extract_project_name, get_default_claude_base_path, get_projects_base_path, get_todos_base_path};
use crate::analysis::ChunkBuilder;
use crate::infrastructure::ContextManager;
use crate::types::domain::DeleteSessionResult;

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
    /// 创建应用状态。
    ///
    /// 必须传入外部共享的 `cache`，确保 AppState（IPC 命令层）与
    /// ServiceContext（文件监听器层）使用同一个缓存实例。
    pub fn new(config_manager: Arc<ConfigManager>, cache: DataCache) -> Self {
        Self { cache, config_manager }
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
/// 扫描项目目录下的 `.jsonl` 文件，构建会话元数据，并按文件修改时间降序排列。
#[command]
pub async fn get_sessions(project_id: String) -> Result<Vec<Session>, String> {
    let base_path = get_projects_base_path();
    let project_dir_name = extract_base_dir(&project_id);
    let project_dir = base_path.join(&project_dir_name);

    if !project_dir.exists() {
        return Ok(vec![]);
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
        .map_err(|e| e.to_string())?;

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

    // 解析子 Agent 数据并链接到 Chunk
    let subagents: Vec<crate::types::chunks::Process> = {
        let resolver = crate::discovery::subagent_resolver::SubagentResolver::new(
            get_projects_base_path(),
            std::sync::Arc::new(crate::infrastructure::fs_provider::LocalFsProvider::new()),
        );
        resolver
            .resolve_subagents(&project_id, &session_id, Some(&parsed.task_calls), Some(&parsed.messages))
            .into_iter()
            .map(Into::into)
            .collect()
    };

    // 使用 ChunkBuilder 将解析后的消息构建为可视化 Chunk
    let detail = ChunkBuilder::build_session_detail(
        session,
        parsed.messages.clone(),
        subagents,
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
/// 扫描 `~/.claude/projects/` 下的所有子目录，返回按最近会话时间降序排列的项目列表。
#[command]
pub async fn get_projects() -> Result<Vec<crate::types::domain::Project>, String> {
    let projects_dir = get_projects_base_path();
    let todos_dir = get_todos_base_path();
    let scanner = ProjectScanner::with_paths(
        projects_dir,
        todos_dir,
        std::sync::Arc::new(crate::infrastructure::fs_provider::LocalFsProvider::new()),
    );
    Ok(scanner.scan())
}

// =============================================================================
// 会话分页命令
// =============================================================================

/// 分页获取指定项目的会话列表。
///
/// 支持基于游标的分页，默认每页 20 条，最大 200 条。
#[command]
pub async fn get_sessions_paginated(
    project_id: String,
    cursor: Option<String>,
    limit: Option<u32>,
    options: Option<SessionsPaginationOptions>,
) -> Result<PaginatedSessionsResult, String> {
    let page_limit = limit.unwrap_or(20).min(200).max(1) as usize;

    let scanner = ProjectScanner::new();
    let all_sessions = scanner.list_sessions(&project_id);

    let opts = options.unwrap_or_default();
    let total_count = if opts.include_total_count.unwrap_or(true) {
        all_sessions.len() as u32
    } else {
        0
    };

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

/// 删除指定会话及其所有关联文件。
///
/// 删除 JSONL 主文件、subagents、tool-results、file-history、todos、debug、
/// session-env、tasks、plans、security_warnings_state 等关联文件，
/// 同时清理 sessions-index.json 和配置中的 pin/hide 记录。
///
/// 仅支持本地上下文，SSH 远程上下文会返回错误。
#[command]
pub async fn delete_session(
    state: State<'_, Arc<RwLock<AppState>>>,
    context_manager: State<'_, Arc<RwLock<ContextManager>>>,
    project_id: String,
    session_id: String,
) -> Result<DeleteSessionResult, String> {
    // Validate session_id is a valid UUID
    if uuid::Uuid::parse_str(&session_id).is_err() {
        return Err(format!("Invalid session_id: '{}'", session_id));
    }

    // Reject SSH contexts — SFTP delete not yet supported
    {
        let mgr = context_manager.read().await;
        if let Some(active_ctx) = mgr.get_active() {
            let ctx = active_ctx.read().await;
            if ctx.context_type == crate::infrastructure::service_context::ContextType::Ssh {
                return Err("远程 session 暂不支持删除".to_string());
            }
        }
    }

    let claude_base = get_default_claude_base_path();
    let base_path = get_projects_base_path();
    let project_dir_name = extract_base_dir(&project_id);
    let project_dir = base_path.join(&project_dir_name);

    let mut main_file_deleted = false;
    let mut associated_deleted = 0u32;
    let mut errors = 0u32;

    // Helper: try delete, log on failure, count results
    async fn try_remove_file(path: &std::path::Path) -> bool {
        if tokio::fs::remove_file(path).await.is_ok() {
            true
        } else {
            false
        }
    }

    async fn try_remove_dir(path: &std::path::Path) -> bool {
        if tokio::fs::remove_dir_all(path).await.is_ok() {
            true
        } else {
            false
        }
    }

    // 1. Delete main JSONL file
    let jsonl_path = project_dir.join(format!("{}.jsonl", session_id));
    if jsonl_path.exists() {
        main_file_deleted = try_remove_file(&jsonl_path).await;
        if main_file_deleted {
            log::info!("Deleted session file: {}", jsonl_path.display());
        } else {
            log::warn!("Failed to delete session file: {}", jsonl_path.display());
            errors += 1;
        }
    }

    // 2. Delete session directory (subagents + tool-results)
    let session_dir = project_dir.join(&session_id);
    if session_dir.exists() {
        if try_remove_dir(&session_dir).await {
            associated_deleted += 1;
            log::info!("Deleted session directory: {}", session_dir.display());
        } else {
            errors += 1;
        }
    }

    // 3. Delete file-history
    let file_history_dir = claude_base.join("file-history").join(&session_id);
    if file_history_dir.exists() {
        if try_remove_dir(&file_history_dir).await {
            associated_deleted += 1;
        } else {
            errors += 1;
        }
    }

    // 4. Delete todos (glob match: {session_id}-*.json)
    let todos_dir = claude_base.join("todos");
    if let Ok(mut entries) = tokio::fs::read_dir(&todos_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&format!("{}-", session_id)) && name.ends_with(".json") {
                if try_remove_file(&entry.path()).await {
                    associated_deleted += 1;
                } else {
                    errors += 1;
                }
            }
        }
    }

    // 5. Delete debug logs (glob match: *{session_id}*.txt)
    let debug_dir = claude_base.join("debug");
    if let Ok(mut entries) = tokio::fs::read_dir(&debug_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.contains(&session_id) && name.ends_with(".txt") {
                if try_remove_file(&entry.path()).await {
                    associated_deleted += 1;
                } else {
                    errors += 1;
                }
            }
        }
    }

    // 6. Delete security_warnings_state
    let security_path = claude_base.join(format!("security_warnings_state_{}.json", session_id));
    if security_path.exists() {
        if try_remove_file(&security_path).await {
            associated_deleted += 1;
        } else {
            errors += 1;
        }
    }

    // 7. Delete session-env
    let session_env_dir = claude_base.join("session-env").join(&session_id);
    if session_env_dir.exists() {
        if try_remove_dir(&session_env_dir).await {
            associated_deleted += 1;
        } else {
            errors += 1;
        }
    }

    // 8. Delete tasks
    let tasks_dir = claude_base.join("tasks").join(&session_id);
    if tasks_dir.exists() {
        if try_remove_dir(&tasks_dir).await {
            associated_deleted += 1;
        } else {
            errors += 1;
        }
    }

    // 9. Delete plans (glob match: *{session_id}*.md)
    let plans_dir = claude_base.join("plans");
    if let Ok(mut entries) = tokio::fs::read_dir(&plans_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.contains(&session_id) && name.ends_with(".md") {
                if try_remove_file(&entry.path()).await {
                    associated_deleted += 1;
                } else {
                    errors += 1;
                }
            }
        }
    }

    // 10. Clean up sessions-index.json entry
    let index_path = project_dir.join("sessions-index.json");
    if index_path.exists() {
        if let Ok(content) = tokio::fs::read_to_string(&index_path).await {
            if let Ok(mut index) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(sessions_arr) = index.get_mut("sessions").and_then(|v| v.as_array_mut()) {
                    let before = sessions_arr.len();
                    sessions_arr.retain(|s| {
                        s.get("sessionId")
                            .or_else(|| s.get("session_id"))
                            .and_then(|v| v.as_str())
                            .map(|id| id != session_id)
                            .unwrap_or(true)
                    });
                    if sessions_arr.len() < before {
                        if let Ok(updated) = serde_json::to_string_pretty(&index) {
                            if tokio::fs::write(&index_path, updated).await.is_ok() {
                                log::info!("Updated sessions-index.json for deleted session {}", session_id);
                            } else {
                                errors += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    // 11. Clean up pin/hide records from ConfigManager
    {
        let app_state = state.read().await;
        app_state.config_manager.unpin_session(project_id.clone(), session_id.clone());
        app_state.config_manager.unhide_session(project_id.clone(), session_id.clone());
    }

    // Invalidate cache for this session
    {
        let app_state = state.read().await;
        app_state.cache.invalidate_session(&project_id, &session_id).await;
    }

    // Small delay to ensure filesystem sync before returning
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    log::info!(
        "Session deleted: {} (main={}, associated={}, errors={})",
        session_id, main_file_deleted, associated_deleted, errors
    );

    Ok(DeleteSessionResult {
        main_file_deleted,
        associated_deleted,
        errors,
    })
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
            std::sync::Arc::new(crate::infrastructure::fs_provider::LocalFsProvider::new()),
        );
        resolver
            .resolve_subagents(&project_id, &session_id, Some(&parsed.task_calls), Some(&parsed.messages))
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
            std::sync::Arc::new(crate::infrastructure::fs_provider::LocalFsProvider::new()),
        );
        resolver
            .resolve_subagents(&project_id, &session_id, Some(&parsed.task_calls), Some(&parsed.messages))
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

        // 跳过命令输出和用户中断消息
        if is_command_output_content(trimmed)
            || trimmed.starts_with("[Request interrupted by user")
        {
            continue;
        }

        // 保存命令名作为备选标题，继续查找真实用户文本。
        // Match Electron's `content.startsWith('<command-name>')` check exactly.
        if trimmed.starts_with("<command-name>") {
            if first_command_text.is_none() {
                first_command_text = extract_command_display(trimmed);
            }
            continue;
        }

        // 找到真实用户文本 — 清理并截断到 500 字符。
        // Note: Electron does NOT check isMeta here; meta messages with text
        // content are used as titles (e.g. skill invocation messages).
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

    // createdAt: use first message timestamp from JSONL, fallback to file birth time.
    // This matches Electron's buildSessionMetadata() behavior for date grouping.
    let birthtime_ms = metadata
        .created()
        .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64)
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
