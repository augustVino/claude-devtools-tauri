//! Project Service — 项目扫描、会话列表、仓库分组。
//!
//! 封装 ProjectScanner 和 WorktreeGrouper 的使用，为 commands 和 routes
//! 提供统一的项目数据访问接口。
//! 持有 `Arc<RwLock<ContextManager>>`，每次方法调用从 active ServiceContext
//! 取 fs_provider / projects_dir / todos_dir，重建无状态 ProjectScanner。

use std::sync::Arc;
use tokio::sync::RwLock;

use crate::discovery::{ProjectScanner, WorktreeGrouper};
use crate::error::AppError;
use crate::infrastructure::ContextManager;
use crate::types::domain::{Project, RepositoryGroup, Session};

/// 项目服务 — 扫描、列出、分组项目与会话（具体实现）。
pub struct ProjectServiceImpl {
    context_manager: Arc<RwLock<ContextManager>>,
}

impl ProjectServiceImpl {
    /// 创建新的 ProjectService。
    pub fn new(context_manager: Arc<RwLock<ContextManager>>) -> Self {
        Self { context_manager }
    }

    /// 从 active ServiceContext 取依赖并构建 ProjectScanner。
    async fn scanner(&self) -> Result<ProjectScanner, AppError> {
        let active_arc = {
            let mgr = self.context_manager.read().await;
            mgr.get_active()
                .ok_or_else(|| AppError::Internal("No active ServiceContext".into()))?
        };
        let ctx = active_arc.read().await;
        Ok(ProjectScanner::with_paths(
            ctx.projects_dir.clone(),
            ctx.todos_dir.clone(),
            ctx.fs_provider.clone(),
        ))
    }
}

#[async_trait::async_trait]
impl super::project_service_trait::ProjectService for ProjectServiceImpl {
    async fn scan_projects(&self) -> Result<Vec<Project>, AppError> {
        let scanner = self.scanner().await?;
        Ok(scanner.scan())
    }

    async fn list_sessions(&self, project_id: &str) -> Result<Vec<Session>, AppError> {
        let scanner = self.scanner().await?;
        Ok(scanner.list_sessions(project_id))
    }

    async fn get_repository_groups(&self) -> Result<Vec<RepositoryGroup>, AppError> {
        let active_arc = {
            let mgr = self.context_manager.read().await;
            mgr.get_active()
                .ok_or_else(|| AppError::Internal("No active ServiceContext".into()))?
        };
        let (projects_dir, scanner) = {
            let ctx = active_arc.read().await;
            let projects_dir = ctx.projects_dir.clone();
            let scanner = ProjectScanner::with_paths(
                ctx.projects_dir.clone(),
                ctx.todos_dir.clone(),
                ctx.fs_provider.clone(),
            );
            (projects_dir, scanner)
        };

        if !projects_dir.exists() {
            return Ok(Vec::new());
        }

        let projects = scanner.scan();
        if projects.is_empty() {
            return Ok(Vec::new());
        }

        let grouper = WorktreeGrouper::new(projects_dir);
        Ok(grouper.group_by_repository(projects))
    }

    async fn get_worktree_sessions(&self, worktree_id: &str) -> Result<Vec<Session>, AppError> {
        self.list_sessions(worktree_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // 引入 trait 使 svc.scan_projects() 等方法在作用域内可见
    use crate::services::ProjectService as _;
    use crate::infrastructure::service_context::{ContextType, ServiceContext, ServiceContextConfig};
    use crate::infrastructure::LocalFsProvider;
    use std::fs;
    use std::path::PathBuf;

    /// 构造含 local context 的 ContextManager，用于 ServiceImpl 测试。
    async fn make_context_manager(
        projects_dir: PathBuf,
        todos_dir: PathBuf,
    ) -> Arc<RwLock<ContextManager>> {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(ServiceContextConfig {
            id: "local".to_string(),
            context_type: ContextType::Local,
            projects_dir,
            todos_dir,
            fs_provider: Arc::new(LocalFsProvider::new()),
            cache: None,
        }))
        .unwrap();
        Arc::new(RwLock::new(mgr))
    }

    /// 构造空 ContextManager（无任何 context 注册），用于测试无 active context 场景。
    fn make_empty_context_manager() -> Arc<RwLock<ContextManager>> {
        Arc::new(RwLock::new(ContextManager::new()))
    }

    fn setup_test_dirs(temp: &tempfile::TempDir) -> (PathBuf, PathBuf) {
        let projects = temp.path().join("projects");
        let todos = temp.path().join("todos");
        fs::create_dir_all(&projects).unwrap();
        fs::create_dir_all(&todos).unwrap();
        (projects, todos)
    }

    #[tokio::test]
    async fn test_scan_projects_empty() {
        let dir = tempfile::tempdir().unwrap();
        let (projects, todos) = setup_test_dirs(&dir);
        let cm = make_context_manager(projects, todos).await;
        let svc = ProjectServiceImpl::new(cm);
        assert!(svc.scan_projects().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_list_sessions_empty_project() {
        let dir = tempfile::tempdir().unwrap();
        let (projects, todos) = setup_test_dirs(&dir);
        let cm = make_context_manager(projects, todos).await;
        let svc = ProjectServiceImpl::new(cm);
        assert!(svc.list_sessions("nonexistent").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_get_repository_groups_empty() {
        let dir = tempfile::tempdir().unwrap();
        let (projects, todos) = setup_test_dirs(&dir);
        let cm = make_context_manager(projects, todos).await;
        let svc = ProjectServiceImpl::new(cm);
        assert!(svc.get_repository_groups().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_get_worktree_sessions_empty() {
        let dir = tempfile::tempdir().unwrap();
        let (projects, todos) = setup_test_dirs(&dir);
        let cm = make_context_manager(projects, todos).await;
        let svc = ProjectServiceImpl::new(cm);
        assert!(svc.get_worktree_sessions("abc").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_scan_projects_returns_error_when_no_active_context() {
        let svc = ProjectServiceImpl::new(make_empty_context_manager());
        let result = svc.scan_projects().await;
        assert!(matches!(result, Err(AppError::Internal(msg)) if msg.contains("No active ServiceContext")));
    }
}
