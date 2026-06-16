//! Project Service Trait — 项目扫描、会话列表、仓库分组的抽象接口。

use async_trait::async_trait;

use crate::error::AppError;
use crate::types::domain::{Project, RepositoryGroup, Session};

#[async_trait]
pub trait ProjectService: Send + Sync {
    async fn scan_projects(&self) -> Result<Vec<Project>, AppError>;
    async fn list_sessions(&self, project_id: &str) -> Result<Vec<Session>, AppError>;
    async fn get_repository_groups(&self) -> Result<Vec<RepositoryGroup>, AppError>;
    async fn get_worktree_sessions(&self, worktree_id: &str) -> Result<Vec<Session>, AppError>;
}
