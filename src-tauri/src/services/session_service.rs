//! Session Service — 会话 CRUD、详情构建、元数据、瀑布图。
//!
//! 核心业务逻辑层：封装 JSONL 解析、缓存读写、Chunk 构建、子 Agent 解析等操作。
//! Tauri commands 和 HTTP routes 都通过此服务访问会话数据。
//!
//! 持有 `Arc<RwLock<ContextManager>>`，每次方法调用从 active ServiceContext
//! 取 fs_provider / projects_dir / cache 等依赖。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::analysis::ChunkBuilder;
use crate::discovery::subagent_resolver::SubagentResolver;
use crate::error::AppError;
use crate::infrastructure::fs_provider::FsProvider;
use crate::infrastructure::{ConfigManager, ContextManager, DataCache};
use crate::agents::{agent_for_path, parse_session_for};
use crate::parsing::ParsedSession;
use crate::types::chunks::{
    ConversationGroup, Process, SessionDetail, SessionDetailResponse, SessionDetailUnchanged,
};
use crate::types::domain::{
    DeleteSessionResult, PaginatedSessionsResult, Session, SessionMetrics,
    SessionsPaginationOptions,
};
use crate::utils::content_sanitizer::{
    extract_command_display, is_command_output_content, sanitize_display_content,
};
use crate::utils::{
    decode_path, extract_base_dir, get_default_claude_base_path,
    pagination::{decode_cursor, encode_cursor},
};
use async_trait::async_trait;

use super::project_service_trait::ProjectService;

/// 会话服务 — 所有会话相关操作的单一入口（具体实现）。
pub struct SessionServiceImpl {
    context_manager: Arc<RwLock<ContextManager>>,
    config_manager: Arc<ConfigManager>,
    project_service: Arc<dyn ProjectService>,
    #[allow(dead_code)]
    repo: Arc<dyn crate::infrastructure::session_repository::SessionRepository>,
}

impl SessionServiceImpl {
    /// 创建新的 SessionService。
    pub fn new(
        context_manager: Arc<RwLock<ContextManager>>,
        config_manager: Arc<ConfigManager>,
        project_service: Arc<dyn ProjectService>,
        repo: Arc<dyn crate::infrastructure::session_repository::SessionRepository>,
    ) -> Self {
        Self {
            context_manager,
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

    /// 从 active context 取 projects_dir。
    async fn projects_dir(&self) -> Result<PathBuf, AppError> {
        let active_arc = {
            let mgr = self.context_manager.read().await;
            mgr.get_active()
                .ok_or_else(|| AppError::Internal("No active ServiceContext".into()))?
        };
        let ctx = active_arc.read().await;
        Ok(ctx.projects_dir.clone())
    }

    /// 从 active context 取 fs_provider。
    ///
    /// 所有路径存在性检查必须通过此 provider 走（对齐 Electron fsProvider.exists），
    /// 不能用 `Path::exists()`（本地 fs，SSH 模式下永远 false）。
    async fn fs_provider(&self) -> Result<Arc<dyn FsProvider>, AppError> {
        let active_arc = {
            let mgr = self.context_manager.read().await;
            mgr.get_active()
                .ok_or_else(|| AppError::Internal("No active ServiceContext".into()))?
        };
        let ctx = active_arc.read().await;
        Ok(ctx.fs_provider.clone())
    }

    /// 通过 fs_provider 检查路径是否存在（SSH-aware）。
    async fn path_exists(&self, path: &Path) -> Result<bool, AppError> {
        let fs_provider = self.fs_provider().await?;
        Ok(fs_provider.exists(path).unwrap_or(false))
    }

    /// 构建项目目录路径。
    async fn project_dir(&self, project_id: &str) -> Result<PathBuf, AppError> {
        let name = extract_base_dir(project_id);
        let projects_dir = self.projects_dir().await?;
        Ok(projects_dir.join(&name))
    }

    /// 构建会话文件路径。
    ///
    /// 先按 claude 布局直推（零开销，旧路径行为不变）；miss 时依次尝试
    /// 其他 agent 的 locate（如 pi：目录编码不同构，枚举匹配）。都不存在时
    /// 返回 claude 直推路径（下游 exists 检查按旧语义处理）。
    async fn session_path(&self, project_id: &str, session_id: &str) -> Result<PathBuf, AppError> {
        let project_dir = self.project_dir(project_id).await?;
        let claude_path = project_dir.join(format!("{}.jsonl", session_id));
        if self.path_exists(&claude_path).await? {
            return Ok(claude_path);
        }
        let fs_provider = self.fs_provider().await?;
        let (projects_dir, home_dir) = {
            let active_arc = {
                let mgr = self.context_manager.read().await;
                mgr.get_active()
                    .ok_or_else(|| AppError::Internal("No active ServiceContext".into()))?
            };
            let ctx = active_arc.read().await;
            (ctx.projects_dir.clone(), ctx.home_dir.clone())
        };
        if let Some(p) = crate::agents::locate_extra_session(
            &projects_dir,
            &home_dir,
            project_id,
            session_id,
            fs_provider.as_ref(),
        ) {
            return Ok(p);
        }
        Ok(claude_path)
    }

    /// 解析子 Agent 数据并转换为 chunks::Process 列表。
    async fn resolve_subagents(
        &self,
        project_id: &str,
        session_id: &str,
        parsed: &ParsedSession,
    ) -> Result<Vec<Process>, AppError> {
        let active_arc = {
            let mgr = self.context_manager.read().await;
            mgr.get_active()
                .ok_or_else(|| AppError::Internal("No active ServiceContext".into()))?
        };
        let (fs_provider, projects_dir) = {
            let ctx = active_arc.read().await;
            (ctx.fs_provider.clone(), ctx.projects_dir.clone())
        };
        let resolver = SubagentResolver::new(projects_dir, fs_provider);
        Ok(resolver
            .resolve_subagents(
                project_id,
                session_id,
                Some(&parsed.task_calls),
                Some(&parsed.messages),
            )
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// 从 JSONL 文件构建会话元数据（与原 build_session_metadata 完全对齐）。
    ///
    /// 解析文件获取首条用户消息作为标题，同时提取创建时间、消息数量、
    /// 子 Agent 标记、Git 分支等元信息。
    pub(crate) async fn build_session_metadata(
        &self,
        path: &Path,
        project_id: &str,
        session_id: &str,
    ) -> Result<Option<Session>, AppError> {
        // SSH-aware: 通过 fs_provider 获取元数据（不能用 path.metadata() 本地 fs）。
        let fs_provider = self.fs_provider().await?;
        let stat = match fs_provider.stat(path) {
            Ok(s) => s,
            Err(_) => return Ok(None),
        };
        let parsed = parse_session_for(path, fs_provider.as_ref());
        Ok(Self::build_session_metadata_inner(
            path,
            project_id,
            session_id,
            &parsed,
            stat.mtime_ms,
            stat.birthtime_ms,
        ))
    }

    /// 纯函数：从已 parse 的 ParsedSession 构建 Session 元数据。
    /// 调用方持有 parsed 时复用，避免重复 parse（SSH 上每次 read 全文 ~秒级）。
    ///
    /// `session_id` 是统一寻址 id（前端 tab/列表用它）：claude 文件 stem 即
    /// id，但 pi 等家的文件名带时间戳前缀，**不能用 file_stem**。
    fn build_session_metadata_inner(
        path: &Path,
        project_id: &str,
        session_id: &str,
        parsed: &ParsedSession,
        mtime_ms: u64,
        birthtime_ms: u64,
    ) -> Option<Session> {
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
        let project_path =
            Self::extract_cwd_from_messages(parsed).unwrap_or_else(|| decode_path(project_id));

        // createdAt: use first message timestamp from JSONL, fallback to file birth time.
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
            id: session_id.to_string(),
            agent: agent_for_path(path),
            project_id: project_id.to_string(),
            project_path,
            created_at,
            updated_at: Some(mtime_ms),
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
    fn fallback_session(
        &self,
        session_id: &str,
        project_id: &str,
        parsed: &ParsedSession,
        path: &Path,
    ) -> Session {
        let fallback_path =
            Self::extract_cwd_from_messages(parsed).unwrap_or_else(|| decode_path(project_id));

        Session {
            id: session_id.to_string(),
            agent: crate::agents::agent_for_path(path),
            project_id: project_id.to_string(),
            project_path: fallback_path,
            created_at: 0,
            updated_at: None,
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
    // 注：原 SessionService::get_sessions 已删除（与 ProjectService::list_sessions
    // 重叠）。完整列表语义统一走 ProjectService::list_sessions（discovery 层）。
    // SessionService 保留分页/批量查询变体（语义不同）和详情类操作。

    /// 分页获取指定项目的会话列表。
    ///
    /// 支持基于游标的分页，默认每页 20 条，最大 200 条。
    pub async fn get_sessions_paginated(
        &self,
        project_id: &str,
        cursor: Option<&str>,
        limit: Option<u32>,
        options: Option<SessionsPaginationOptions>,
    ) -> Result<PaginatedSessionsResult, AppError> {
        let page_limit = limit.unwrap_or(20).min(200).max(1) as usize;
        let all_sessions = self.project_service.list_sessions(project_id).await?;

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
    ) -> Result<Vec<Session>, AppError> {
        const MAX_SESSION_IDS: usize = 50;
        let id_set: HashSet<String> = session_ids.iter().take(MAX_SESSION_IDS).cloned().collect();

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

        let all_sessions = self.project_service.list_sessions(project_id).await?;
        Ok(all_sessions
            .into_iter()
            .filter(|s| id_set.contains(&s.id))
            .collect())
    }

    // ════════════════════════════════════════════════════════════════
    //  会话详情
    // ════════════════════════════════════════════════════════════════

    /// 获取完整会话详情（含原始消息），专供导出功能使用。
    ///
    /// 不走 slim 缓存，每次重新解析以确保数据完整性。
    pub async fn get_session_detail_for_export(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<Option<SessionDetail>, AppError> {
        // 始终重新解析，不使用 slim 缓存
        let session_path = self.session_path(project_id, session_id).await?;
        let fs_provider = self.fs_provider().await?;
        if !fs_provider.exists(&session_path).unwrap_or(false) {
            return Ok(None);
        }

        let parsed = parse_session_for(&session_path, fs_provider.as_ref());
        // 复用 parsed 避免重复读全文（SSH 上每次 read ~秒级）
        let stat = fs_provider.stat(&session_path).ok();
        let session = Self::build_session_metadata_inner(
            &session_path,
            project_id,
            session_id,
            &parsed,
            stat.as_ref().map(|s| s.mtime_ms).unwrap_or(0),
            stat.as_ref().map(|s| s.birthtime_ms).unwrap_or(0),
        )
        .unwrap_or_else(|| self.fallback_session(session_id, project_id, &parsed, &session_path));

        let subagents = self
            .resolve_subagents(project_id, session_id, &parsed)
            .await?;
        let detail =
            ChunkBuilder::build_session_detail(session, parsed.messages.clone(), subagents);
        // 注意：此处不清空 process.messages —— 导出需要完整数据

        Ok(Some(detail))
    }

    /// 获取指定会话的完整详情（含可视化 Chunk 数据）。
    ///
    /// 优先从缓存读取，缓存未命中时解析 JSONL 文件并通过 ChunkBuilder 构建详情。
    /// 支持基于文件 mtime+size 的 fingerprint 短路：当已知 fingerprint 匹配时返回 `Unchanged`。
    pub async fn get_session_detail(
        &self,
        project_id: &str,
        session_id: &str,
        known_fingerprint: Option<&str>,
    ) -> Result<Option<SessionDetailResponse>, AppError> {
        let session_path = self.session_path(project_id, session_id).await?;
        if !self.path_exists(&session_path).await? {
            return Ok(None);
        }

        let (cache, fs_provider) = {
            let active_arc = {
                let mgr = self.context_manager.read().await;
                mgr.get_active()
                    .ok_or_else(|| AppError::Internal("No active ServiceContext".into()))?
            };
            let ctx = active_arc.read().await;
            (ctx.cache.clone(), ctx.fs_provider.clone())
        };

        let fingerprint = fs_provider
            .stat(&session_path)
            .ok()
            .map(|s| format!("{}-{}", s.mtime_ms, s.size));

        if let (Some(known), Some(ref current)) = (known_fingerprint, &fingerprint) {
            if known == current {
                return Ok(Some(SessionDetailResponse::Unchanged(
                    SessionDetailUnchanged {
                        unchanged: true,
                        fingerprint: current.clone(),
                    },
                )));
            }
        }

        if let Some(cached) = cache
            .get_session(project_id, session_id, fingerprint.as_deref())
            .await
        {
            return Ok(Some(SessionDetailResponse::Full(serde_json::from_value(
                cached,
            )?)));
        }

        let parsed = parse_session_for(&session_path, fs_provider.as_ref());
        // 复用 parsed：build_session_metadata_inner 不再二次 parse（避免 SSH 上重复读全文）
        let stat = fs_provider.stat(&session_path).ok();
        let session = Self::build_session_metadata_inner(
            &session_path,
            project_id,
            session_id,
            &parsed,
            stat.as_ref().map(|s| s.mtime_ms).unwrap_or(0),
            stat.as_ref().map(|s| s.birthtime_ms).unwrap_or(0),
        )
        .unwrap_or_else(|| self.fallback_session(session_id, project_id, &parsed, &session_path));

        let subagents = self
            .resolve_subagents(project_id, session_id, &parsed)
            .await?;
        let mut detail =
            ChunkBuilder::build_session_detail(session, parsed.messages.clone(), subagents);

        detail.messages.clear();
        for process in &mut detail.processes {
            process.messages.clear();
        }

        detail.fingerprint = fingerprint.clone();

        let value = serde_json::to_value(&detail)?;

        cache
            .set_session(project_id, session_id, value, fingerprint.as_deref())
            .await;

        Ok(Some(SessionDetailResponse::Full(detail)))
    }

    /// 获取指定会话的指标数据（消息数、token 用量等）。
    pub async fn get_session_metrics(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<Option<SessionMetrics>, AppError> {
        let session_path = self.session_path(project_id, session_id).await?;
        let fs_provider = self.fs_provider().await?;
        if !fs_provider.exists(&session_path).unwrap_or(false) {
            return Ok(None);
        }

        let parsed = parse_session_for(&session_path, fs_provider.as_ref());
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
    ) -> Result<Vec<ConversationGroup>, AppError> {
        let session_path = self.session_path(project_id, session_id).await?;
        let fs_provider = self.fs_provider().await?;
        if !fs_provider.exists(&session_path).unwrap_or(false) {
            return Ok(vec![]);
        }

        let parsed = parse_session_for(&session_path, fs_provider.as_ref());
        let subagents = self
            .resolve_subagents(project_id, session_id, &parsed)
            .await?;

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
    ) -> Result<Option<crate::analysis::waterfall_builder::WaterfallData>, AppError> {
        let session_path = self.session_path(project_id, session_id).await?;
        let fs_provider = self.fs_provider().await?;
        if !fs_provider.exists(&session_path).unwrap_or(false) {
            return Ok(None);
        }

        let parsed = parse_session_for(&session_path, fs_provider.as_ref());
        // 复用 parsed 避免重复读全文
        let stat = fs_provider.stat(&session_path).ok();
        let session = Self::build_session_metadata_inner(
            &session_path,
            project_id,
            session_id,
            &parsed,
            stat.as_ref().map(|s| s.mtime_ms).unwrap_or(0),
            stat.as_ref().map(|s| s.birthtime_ms).unwrap_or(0),
        )
        .unwrap_or_else(|| self.fallback_session(session_id, project_id, &parsed, &session_path));

        let subagents = self
            .resolve_subagents(project_id, session_id, &parsed)
            .await?;
        let detail =
            ChunkBuilder::build_session_detail(session, parsed.messages.clone(), subagents);
        let waterfall = crate::analysis::waterfall_builder::build_waterfall_data(
            &detail.chunks,
            &detail.processes,
        );
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
    ) -> Result<DeleteSessionResult, AppError> {
        // Validate UUID
        if uuid::Uuid::parse_str(session_id).is_err() {
            return Err(AppError::InvalidInput(format!(
                "Invalid session_id: '{}'",
                session_id
            )));
        }

        let claude_base = get_default_claude_base_path();
        let project_dir = self.project_dir(project_id).await?;

        // 拿 cache 用于失效（与原 self.cache 一致）
        let cache: DataCache = {
            let active_arc = {
                let mgr = self.context_manager.read().await;
                mgr.get_active()
                    .ok_or_else(|| AppError::Internal("No active ServiceContext".into()))?
            };
            let ctx = active_arc.read().await;
            ctx.cache.clone()
        };

        let mut main_file_deleted = false;
        let mut associated_deleted = 0u32;
        let mut errors = 0u32;

        async fn try_remove_file(path: &Path) -> bool {
            tokio::fs::remove_file(path).await.is_ok()
        }
        async fn try_remove_dir(path: &Path) -> bool {
            tokio::fs::remove_dir_all(path).await.is_ok()
        }

        // 1. Main JSONL file — 统一寻址（claude 直推 → 其他 agent locate
        //    fallback），pi 等家的会话删除不再落空。注意删除本身是本地
        // fs 能力（tokio::fs），SSH 模式下与既有行为一致地不生效。
        let jsonl_path = self.session_path(project_id, session_id).await?;
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
        let sec_path = claude_base.join(format!("security_warnings_state_{}.json", session_id));
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
        let _ = self
            .config_manager
            .unpin_session(project_id.to_string(), session_id.to_string())
            .await;
        let _ = self
            .config_manager
            .unhide_session(project_id.to_string(), session_id.to_string())
            .await;

        // Invalidate cache
        cache.invalidate_session(project_id, session_id).await;

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

// ════════════════════════════════════════════════════════════════
//  Trait Implementation
// ════════════════════════════════════════════════════════════════

#[async_trait]
impl super::session_service_trait::SessionService for SessionServiceImpl {

    async fn get_session_detail(
        &self,
        project_id: &str,
        session_id: &str,
        known_fingerprint: Option<&str>,
    ) -> Result<Option<crate::types::chunks::SessionDetailResponse>, AppError> {
        self.get_session_detail(project_id, session_id, known_fingerprint)
            .await
    }

    async fn get_session_detail_for_export(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<Option<crate::types::chunks::SessionDetail>, AppError> {
        self.get_session_detail_for_export(project_id, session_id)
            .await
    }

    async fn get_sessions_paginated(
        &self,
        project_id: &str,
        cursor: Option<&str>,
        limit: Option<u32>,
        options: Option<crate::types::domain::SessionsPaginationOptions>,
    ) -> Result<crate::types::domain::PaginatedSessionsResult, AppError> {
        self.get_sessions_paginated(project_id, cursor, limit, options)
            .await
    }

    async fn get_sessions_by_ids(
        &self,
        project_id: &str,
        session_ids: &[String],
    ) -> Result<Vec<crate::types::domain::Session>, AppError> {
        self.get_sessions_by_ids(project_id, session_ids).await
    }

    async fn get_session_metrics(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<Option<crate::types::domain::SessionMetrics>, AppError> {
        self.get_session_metrics(project_id, session_id).await
    }

    async fn get_session_groups(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<Vec<crate::types::chunks::ConversationGroup>, AppError> {
        self.get_session_groups(project_id, session_id).await
    }

    async fn get_waterfall_data(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<Option<crate::analysis::waterfall_builder::WaterfallData>, AppError> {
        self.get_waterfall_data(project_id, session_id).await
    }

    async fn delete_session(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<crate::types::domain::DeleteSessionResult, AppError> {
        self.delete_session(project_id, session_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // 引入 trait 使 svc.get_sessions() 方法在作用域内可见
    use crate::services::SessionService as _;
    use std::path::PathBuf;

    /// 构造空 ContextManager（无任何 context 注册），用于测试无 active context 场景。
    fn make_empty_context_manager() -> Arc<RwLock<ContextManager>> {
        Arc::new(RwLock::new(ContextManager::new()))
    }
}
