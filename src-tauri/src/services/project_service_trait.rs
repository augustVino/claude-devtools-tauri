//! Project Service Trait — 项目扫描、会话列表、仓库分组的抽象接口。

use crate::types::domain::{Project, RepositoryGroup, Session};

pub trait ProjectService: Send + Sync {
    fn scan_projects(&self) -> Vec<Project>;
    fn list_sessions(&self, project_id: &str) -> Vec<Session>;
    fn get_repository_groups(&self) -> Vec<RepositoryGroup>;
    fn get_worktree_sessions(&self, worktree_id: &str) -> Vec<Session>;
}
