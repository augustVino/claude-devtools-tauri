//! Session Service Trait — 会话 CRUD、详情构建、元数据、瀑布图的抽象接口。

use async_trait::async_trait;
use crate::error::AppError;
use crate::types::domain::{
    DeleteSessionResult, PaginatedSessionsResult, Session, SessionMetrics,
    SessionsPaginationOptions,
};
use crate::types::chunks::{ConversationGroup, SessionDetail};

#[async_trait]
pub trait SessionService: Send + Sync {
    async fn get_sessions(&self, project_id: &str) -> Result<Vec<Session>, AppError>;
    async fn get_session_detail(&self, project_id: &str, session_id: &str) -> Result<Option<SessionDetail>, AppError>;
    async fn get_sessions_paginated(&self, project_id: &str, cursor: Option<&str>, limit: Option<u32>, options: Option<SessionsPaginationOptions>) -> Result<PaginatedSessionsResult, AppError>;
    async fn get_sessions_by_ids(&self, project_id: &str, session_ids: &[String]) -> Result<Vec<Session>, AppError>;
    async fn get_session_metrics(&self, project_id: &str, session_id: &str) -> Result<Option<SessionMetrics>, AppError>;
    async fn get_session_groups(&self, project_id: &str, session_id: &str) -> Result<Vec<ConversationGroup>, AppError>;
    async fn get_waterfall_data(&self, project_id: &str, session_id: &str) -> Result<Option<crate::analysis::waterfall_builder::WaterfallData>, AppError>;
    async fn delete_session(&self, project_id: &str, session_id: &str) -> Result<DeleteSessionResult, AppError>;
}
