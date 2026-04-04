//! Tauri IPC 会话命令 — 薄封装层。
//!
//! 所有业务逻辑委托给 [`SessionService`](crate::services::SessionService)。
//! 本模块仅负责参数接收和 State 注入。

use tauri::{command, State};
use std::sync::Arc;

use crate::services::SessionService;
use crate::types::domain::{DeleteSessionResult, PaginatedSessionsResult, SessionsPaginationOptions, Project, Session, SessionMetrics};
use crate::types::chunks::{ConversationGroup, SessionDetail};
use crate::infrastructure::ContextManager;

// AppState is defined in commands/mod.rs — re-exported here for backward compat
pub use super::AppState;

// =============================================================================
// 会话列表命令
// =============================================================================

#[command]
pub async fn get_sessions(
    service: State<'_, Arc<SessionService>>,
    project_id: String,
) -> Result<Vec<Session>, String> {
    service.get_sessions(&project_id).await
}

#[command]
pub async fn get_sessions_paginated(
    service: State<'_, Arc<SessionService>>,
    project_id: String,
    cursor: Option<String>,
    limit: Option<u32>,
    options: Option<SessionsPaginationOptions>,
) -> Result<PaginatedSessionsResult, String> {
    service.get_sessions_paginated(&project_id, cursor.as_deref(), limit, options).await
}

#[command]
pub async fn get_sessions_by_ids(
    service: State<'_, Arc<SessionService>>,
    project_id: String,
    session_ids: Vec<String>,
) -> Result<Vec<Session>, String> {
    service.get_sessions_by_ids(&project_id, &session_ids).await
}

// =============================================================================
// 会话详情命令
// =============================================================================

#[command]
pub async fn get_session_detail(
    service: State<'_, Arc<SessionService>>,
    project_id: String,
    session_id: String,
) -> Result<Option<SessionDetail>, String> {
    service.get_session_detail(&project_id, &session_id).await
}

#[command]
pub async fn get_session_metrics(
    service: State<'_, Arc<SessionService>>,
    project_id: String,
    session_id: String,
) -> Result<Option<SessionMetrics>, String> {
    service.get_session_metrics(&project_id, &session_id).await
}

// =============================================================================
// 派生数据命令
// =============================================================================

#[command]
pub async fn get_session_groups(
    service: State<'_, Arc<SessionService>>,
    project_id: String,
    session_id: String,
) -> Result<Vec<ConversationGroup>, String> {
    service.get_session_groups(&project_id, &session_id).await
}

#[command]
pub async fn get_waterfall_data(
    service: State<'_, Arc<SessionService>>,
    project_id: String,
    session_id: String,
) -> Result<Option<crate::analysis::waterfall_builder::WaterfallData>, String> {
    service.get_waterfall_data(&project_id, &session_id).await
}

// =============================================================================
// 会话管理命令
// =============================================================================

/// 删除会话 — SSH 上下文检查保留在命令层（SSH 删除暂不支持）。
#[command]
pub async fn delete_session(
    service: State<'_, Arc<SessionService>>,
    context_manager: State<'_, Arc<tokio::sync::RwLock<ContextManager>>>,
    project_id: String,
    session_id: String,
) -> Result<DeleteSessionResult, String> {
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

    service.delete_session(&project_id, &session_id).await
}

// =============================================================================
// 项目命令（委托给 ProjectService）
// =============================================================================

#[command]
pub async fn get_projects(
    service: State<'_, Arc<crate::services::ProjectService>>,
) -> Result<Vec<Project>, String> {
    Ok(service.scan_projects())
}
