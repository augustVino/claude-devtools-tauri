//! Session Service — 会话 CRUD、详情构建、元数据、瀑布图。
//!
//! 核心业务逻辑层：封装 JSONL 解析、缓存读写、Chunk 构建、子 Agent 解析等操作。
//! Tauri commands 和 HTTP routes 都通过此服务访问会话数据。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::analysis::ChunkBuilder;
use crate::discovery::subagent_resolver::SubagentResolver;
use crate::infrastructure::{
    ConfigManager, DataCache,
    fs_provider::FsProvider,
};
use crate::parsing::{parse_session_file, ParsedSession};
use crate::types::chunks::{ConversationGroup, Process, SessionDetail};
use crate::types::domain::{
    DeleteSessionResult, PaginatedSessionsResult, Session, SessionMetrics,
    SessionsPaginationOptions,
};
use crate::utils::content_sanitizer::{
    extract_command_display, is_command_output_content,
    sanitize_display_content,
};
use crate::utils::{
    decode_path, extract_base_dir, get_default_claude_base_path,
    pagination::{decode_cursor, encode_cursor},
};

use super::project_service::ProjectService;

/// 会话服务 — 所有会话相关操作的单一入口。
pub struct SessionService {
    fs_provider: Arc<dyn FsProvider>,
    cache: DataCache,
    projects_dir: PathBuf,
    #[allow(dead_code)]
    todos_dir: PathBuf,
    config_manager: Arc<ConfigManager>,
    project_service: Arc<ProjectService>,
    #[allow(dead_code)]
    repo: Arc<dyn crate::infrastructure::session_repository::SessionRepository>,
}

impl SessionService {
    /// 创建新的 SessionService。
    pub fn new(
        fs_provider: Arc<dyn FsProvider>,
        cache: DataCache,
        projects_dir: PathBuf,
        todos_dir: PathBuf,
        config_manager: Arc<ConfigManager>,
        project_service: Arc<ProjectService>,
        repo: Arc<dyn crate::infrastructure::session_repository::SessionRepository>,
    ) -> Self {
        Self {
            fs_provider,
            cache,
            projects_dir,
            todos_dir,
            config_manager,
            project_service,
            repo,
        }
    }

    // ════════════════════════════════════════════════════════════════
    //  内部辅助方法
    // ════════════════════════════════════════════════════════════════

    /// 从解析后的消息中提取第一条非空 cwd。
    ///
    /// 避免使用有损的 decode_path 处理含连字符的项目名（如 "obsidian-stories"）。
    pub(crate) fn extract_cwd_from_messages(parsed: &ParsedSession) -> Option<String> {
        for msg in &parsed.messages {
            if let Some(ref cwd) = msg.cwd {
                if !cwd.is_empty() {
                    return Some(cwd.clone());
                }
            }
        }
        None
    }

    /// 构建项目目录路径。
    fn project_dir(&self, project_id: &str) -> PathBuf {
        let name = extract_base_dir(project_id);
        self.projects_dir.join(&name)
    }

    /// 构建会话文件路径。
    fn session_path(&self, project_id: &str, session_id: &str) -> PathBuf {
        self.project_dir(project_id).join(format!("{}.jsonl", session_id))
    }

    /// 解析子 Agent 数据并转换为 chunks::Process 列表。
    async fn resolve_subagents(
        &self,
        project_id: &str,
        session_id: &str,
        parsed: &ParsedSession,
    ) -> Vec<Process> {
        let resolver = SubagentResolver::new(
            self.projects_dir.clone(),
            self.fs_provider.clone(),
        );
        resolver
            .resolve_subagents(project_id, session_id, Some(&parsed.task_calls), Some(&parsed.messages))
            .into_iter()
            .map(Into::into)
            .collect()
    }

    /// 从 JSONL 文件构建会话元数据（与原 build_session_metadata 完全对齐）。
    ///
    /// 解析文件获取首条用户消息作为标题，同时提取创建时间、消息数量、
    /// 子 Agent 标记、Git 分支等元信息。
    pub(crate) async fn build_session_metadata(
        &self,
        path: &Path,
        project_id: &str,
    ) -> Option<Session> {
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

            // 跳过命令输出和用户中断消息
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

        // Prefer cwd from session file over lossy decode_path (handles dashes in project names)
        let project_path = Self::extract_cwd_from_messages(&parsed)
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

    /// 构建回退 Session（文件不存在时使用）。
    fn fallback_session(&self, session_id: &str, project_id: &str, parsed: &ParsedSession) -> Session {
        let fallback_path = Self::extract_cwd_from_messages(parsed)
            .unwrap_or_else(|| decode_path(project_id));

        Session {
            id: session_id.to_string(),
            project_id: project_id.to_string(),
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
        }
    }

    // ════════════════════════════════════════════════════════════════
    //  会话列表
    // ════════════════════════════════════════════════════════════════

    /// 获取指定项目下的所有会话列表。
    ///
    /// 扫描项目目录下的 `.jsonl` 文件，构建会话元数据，并按文件修改时间降序排列。
    pub async fn get_sessions(&self, project_id: &str) -> Result<Vec<Session>, String> {
        let project_dir = self.project_dir(project_id);

        if !project_dir.exists() {
            return Ok(Vec::new());
        }

        struct FileEntry {
            path: PathBuf,
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
                    .and_then(|t: std::time::SystemTime| {
                        t.duration_since(std::time::UNIX_EPOCH).ok()
                    })
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
        for entry in &file_entries {
            if let Some(session) = self.build_session_metadata(&entry.path, project_id).await {
                sessions.push(session);
            }
        }

        Ok(sessions)
    }

    /// 分页获取指定项目的会话列表。
    ///
    /// 支持基于游标的分页，默认每页 20 条，最大 200 条。
    pub async fn get_sessions_paginated(
        &self,
        project_id: &str,
        cursor: Option<&str>,
        limit: Option<u32>,
        options: Option<SessionsPaginationOptions>,
    ) -> Result<PaginatedSessionsResult, String> {
        let page_limit = limit.unwrap_or(20).min(200).max(1) as usize;
        let all_sessions = self.project_service.list_sessions(project_id);

        let opts = options.unwrap_or_default();
        let total_count = if opts.include_total_count.unwrap_or(true) {
            all_sessions.len() as u32
        } else {
            0
        };

        // 定位游标位置（对齐 Electron：不含 cursor 对应项）
        let start_idx = if let Some(c) = cursor {
            let (_, session_id) = decode_cursor(c);
            all_sessions
                .iter()
                .position(|s| s.id == session_id)
                .map(|pos| pos + 1)
                .unwrap_or(0)
        } else {
            0
        };

        // 截取当前页数据
        let end_idx = (start_idx + page_limit).min(all_sessions.len());
        let sessions = all_sessions[start_idx..end_idx].to_vec();

        let has_more = end_idx < all_sessions.len();
        let next_cursor = if has_more {
            sessions.last().map(|s| encode_cursor(s.created_at, &s.id))
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

    /// 根据 ID 列表批量获取会话。
    ///
    /// 与 HTTP 路由对齐：限制最多 50 个 ID（防御性编程）。
    pub async fn get_sessions_by_ids(
        &self,
        project_id: &str,
        session_ids: &[String],
    ) -> Result<Vec<Session>, String> {
        const MAX_SESSION_IDS: usize = 50;
        let id_set: HashSet<String> = session_ids
            .iter()
            .take(MAX_SESSION_IDS)
            .cloned()
            .collect();

        if session_ids.len() > MAX_SESSION_IDS {
            log::warn!(
                "get_sessions_by_ids: {} IDs requested, capping to {}",
                session_ids.len(),
                MAX_SESSION_IDS
            );
        }

        if id_set.is_empty() {
            return Ok(Vec::new());
        }

        let all_sessions = self.project_service.list_sessions(project_id);
        Ok(all_sessions.into_iter().filter(|s| id_set.contains(&s.id)).collect())
    }

    // ════════════════════════════════════════════════════════════════
    //  会话详情
    // ════════════════════════════════════════════════════════════════

    /// 获取指定会话的完整详情（含可视化 Chunk 数据）。
    ///
    /// 优先从缓存读取，缓存未命中时解析 JSONL 文件并通过 ChunkBuilder 构建详情。
    pub async fn get_session_detail(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<Option<SessionDetail>, String> {
        // 缓存查询
        if let Some(cached) = self.cache.get_session(project_id, session_id).await {
            if let Ok(detail) = serde_json::from_value(cached) {
                return Ok(Some(detail));
            }
        }

        let session_path = self.session_path(project_id, session_id);
        if !session_path.exists() {
            return Ok(None);
        }

        let parsed = parse_session_file(&session_path).await;
        let session = self
            .build_session_metadata(&session_path, project_id)
            .await
            .unwrap_or_else(|| self.fallback_session(session_id, project_id, &parsed));

        let subagents = self.resolve_subagents(project_id, session_id, &parsed).await;
        let detail =
            ChunkBuilder::build_session_detail(session, parsed.messages.clone(), subagents);

        // 写入缓存
        self.cache
            .set_session(
                project_id,
                session_id,
                serde_json::to_value(&detail).unwrap_or_default(),
            )
            .await;

        Ok(Some(detail))
    }

    /// 获取指定会话的指标数据（消息数、token 用量等）。
    pub async fn get_session_metrics(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<Option<SessionMetrics>, String> {
        let session_path = self.session_path(project_id, session_id);
        if !session_path.exists() {
            return Ok(None);
        }

        let parsed = parse_session_file(&session_path).await;
        Ok(Some(parsed.metrics))
    }

    // ════════════════════════════════════════════════════════════════
    //  派生数据
    // ════════════════════════════════════════════════════════════════

    /// 获取会话的对话分组信息。
    ///
    /// 将会话消息按照对话结构分组，同时解析子 Agent 进程数据。
    pub async fn get_session_groups(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<Vec<ConversationGroup>, String> {
        let session_path = self.session_path(project_id, session_id);
        if !session_path.exists() {
            return Ok(vec![]);
        }

        let parsed = parse_session_file(&session_path).await;
        let subagents = self.resolve_subagents(project_id, session_id, &parsed).await;

        Ok(crate::analysis::conversation_group_builder::build_groups(
            &parsed.messages,
            &subagents,
        ))
    }

    /// 获取会话的瀑布图数据（工具调用时序可视化）。
    pub async fn get_waterfall_data(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<Option<crate::analysis::waterfall_builder::WaterfallData>, String>
    {
        let session_path = self.session_path(project_id, session_id);
        if !session_path.exists() {
            return Ok(None);
        }

        let parsed = parse_session_file(&session_path).await;
        let session = self
            .build_session_metadata(&session_path, project_id)
            .await
            .unwrap_or_else(|| self.fallback_session(session_id, project_id, &parsed));

        let subagents = self.resolve_subagents(project_id, session_id, &parsed).await;
        let detail =
            ChunkBuilder::build_session_detail(session, parsed.messages.clone(), subagents);
        let waterfall =
            crate::analysis::waterfall_builder::build_waterfall_data(&detail.chunks, &detail.processes);
        Ok(Some(waterfall))
    }

    // ════════════════════════════════════════════════════════════════
    //  会话管理
    // ════════════════════════════════════════════════════════════════

    /// 删除指定会话及其所有关联文件。
    ///
    /// 删除 JSONL 主文件、subagents、tool-results、file-history、todos、debug、
    /// session-env、tasks、plans、security_warnings_state 等关联文件，
    /// 同时清理 sessions-index.json 和配置中的 pin/hide 记录。
    pub async fn delete_session(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<DeleteSessionResult, String> {
        // Validate UUID
        if uuid::Uuid::parse_str(session_id).is_err() {
            return Err(format!("Invalid session_id: '{}'", session_id));
        }

        let claude_base = get_default_claude_base_path();
        let project_dir = self.project_dir(project_id);

        let mut main_file_deleted = false;
        let mut associated_deleted = 0u32;
        let mut errors = 0u32;

        async fn try_remove_file(path: &Path) -> bool {
            tokio::fs::remove_file(path).await.is_ok()
        }
        async fn try_remove_dir(path: &Path) -> bool {
            tokio::fs::remove_dir_all(path).await.is_ok()
        }

        // 1. Main JSONL file
        let jsonl_path = project_dir.join(format!("{}.jsonl", session_id));
        if jsonl_path.exists() {
            if try_remove_file(&jsonl_path).await {
                main_file_deleted = true;
                log::info!("Deleted session file: {}", jsonl_path.display());
            } else {
                log::warn!("Failed to delete session file: {}", jsonl_path.display());
                errors += 1;
            }
        }

        // 2. Session directory (subagents + tool-results)
        let session_dir = project_dir.join(session_id);
        if session_dir.exists() {
            if try_remove_dir(&session_dir).await {
                associated_deleted += 1;
                log::info!("Deleted session directory: {}", session_dir.display());
            } else {
                errors += 1;
            }
        }

        // 3. file-history
        let fh_dir = claude_base.join("file-history").join(session_id);
        if fh_dir.exists() {
            if try_remove_dir(&fh_dir).await {
                associated_deleted += 1;
            } else {
                errors += 1;
            }
        }

        // 4. Todos (glob match: {session_id}-*.json)
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

        // 5. Debug logs (glob match: *{session_id}*.txt)
        let debug_dir = claude_base.join("debug");
        if let Ok(mut entries) = tokio::fs::read_dir(&debug_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.contains(session_id) && name.ends_with(".txt") {
                    if try_remove_file(&entry.path()).await {
                        associated_deleted += 1;
                    } else {
                        errors += 1;
                    }
                }
            }
        }

        // 6. security_warnings_state
        let sec_path = claude_base.join(format!(
            "security_warnings_state_{}.json",
            session_id
        ));
        if sec_path.exists() {
            if try_remove_file(&sec_path).await {
                associated_deleted += 1;
            } else {
                errors += 1;
            }
        }

        // 7. session-env
        let env_dir = claude_base.join("session-env").join(session_id);
        if env_dir.exists() {
            if try_remove_dir(&env_dir).await {
                associated_deleted += 1;
            } else {
                errors += 1;
            }
        }

        // 8. tasks
        let tasks_dir = claude_base.join("tasks").join(session_id);
        if tasks_dir.exists() {
            if try_remove_dir(&tasks_dir).await {
                associated_deleted += 1;
            } else {
                errors += 1;
            }
        }

        // 9. Plans (glob match: *{session_id}*.md)
        let plans_dir = claude_base.join("plans");
        if let Ok(mut entries) = tokio::fs::read_dir(&plans_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.contains(session_id) && name.ends_with(".md") {
                    if try_remove_file(&entry.path()).await {
                        associated_deleted += 1;
                    } else {
                        errors += 1;
                    }
                }
            }
        }

        // 10. Clean up sessions-index.json
        let index_path = project_dir.join("sessions-index.json");
        if index_path.exists() {
            if let Ok(content) = tokio::fs::read_to_string(&index_path).await {
                if let Ok(mut index) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(sessions_arr) =
                        index.get_mut("sessions").and_then(|v| v.as_array_mut())
                    {
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
                                    log::info!(
                                        "Updated sessions-index.json for deleted session {}",
                                        session_id
                                    );
                                } else {
                                    errors += 1;
                                }
                            }
                        }
                    }
                }
            }
        }

        // 11. Clean up pin/hide from ConfigManager
        let _ = self.config_manager
            .unpin_session(project_id.to_string(), session_id.to_string()).await;
        let _ = self.config_manager
            .unhide_session(project_id.to_string(), session_id.to_string()).await;

        // Invalidate cache
        self.cache.invalidate_session(project_id, session_id).await;

        // Small delay to ensure filesystem sync before returning
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        log::info!(
            "Session deleted: {} (main={}, associated={}, errors={})",
            session_id,
            main_file_deleted,
            associated_deleted,
            errors
        );

        Ok(DeleteSessionResult {
            main_file_deleted,
            associated_deleted,
            errors,
        })
    }
}
