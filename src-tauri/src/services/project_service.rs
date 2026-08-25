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
    /// 从 active ServiceContext 取 fs_provider / projects_dir / home_dir
    /// （多 agent 聚合用；home 为空时聚合层自动降级为仅 claude）。
    async fn context_deps(
        &self,
    ) -> Result<
        (
            std::sync::Arc<dyn crate::infrastructure::fs_provider::FsProvider>,
            std::path::PathBuf,
            std::path::PathBuf,
        ),
        AppError,
    > {
        let active_arc = {
            let mgr = self.context_manager.read().await;
            mgr.get_active()
                .ok_or_else(|| AppError::Internal("No active ServiceContext".into()))?
        };
        let ctx = active_arc.read().await;
        Ok((
            ctx.fs_provider.clone(),
            ctx.projects_dir.clone(),
            ctx.home_dir.clone(),
        ))
    }
}

#[async_trait::async_trait]
impl super::project_service_trait::ProjectService for ProjectServiceImpl {
    async fn scan_projects(&self) -> Result<Vec<Project>, AppError> {
        let scanner = self.scanner().await?;
        let projects = scanner.scan_async().await;
        // 多 agent 聚合：额外 agent（Pi 起）的项目归并进 claude 结果。
        // 聚合是同步阻塞 IO（SFTP read），包 spawn_blocking 对齐 scan_async
        // 的既有模式（避免在 async 上下文串行阻塞触发 IPC 超时）
        let (fs, home) = {
            let (fs, _projects_dir, home) = self.context_deps().await?;
            (fs, home)
        };
        let merged = tokio::task::spawn_blocking(move || {
            crate::agents::merge_extra_projects(projects, fs.as_ref(), &home)
        })
        .await
        .map_err(|e| AppError::Internal(format!("scan aggregation join error: {e}")))?;
        Ok(merged)
    }

    async fn list_sessions(&self, project_id: &str) -> Result<Vec<Session>, AppError> {
        let scanner = self.scanner().await?;
        let sessions = scanner.list_sessions_async(project_id).await;
        let (fs, projects_dir, home) = self.context_deps().await?;
        let project_id = project_id.to_string();
        let appended = tokio::task::spawn_blocking(move || {
            crate::agents::append_extra_sessions(sessions, fs.as_ref(), &projects_dir, &home, &project_id)
        })
        .await
        .map_err(|e| AppError::Internal(format!("list aggregation join error: {e}")))?;
        Ok(appended)
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

        // scan_async 内部已用 fs_provider.exists 检查 projects_dir，
        // 替代原 Path::exists()（本地 fs，SSH 模式下永远 false）。
        let mut projects = scanner.scan_async().await;
        // 多 agent：分组视图与项目列表必须给出一致的「有哪些项目」答案，
        // 同样走归并（cwd 匹配，见 agents 模块文档）
        {
            let (fs, home) = {
                let (fs, _projects_dir, home) = self.context_deps().await?;
                (fs, home)
            };
            projects = tokio::task::spawn_blocking(move || {
                crate::agents::merge_extra_projects(projects, fs.as_ref(), &home)
            })
            .await
            .map_err(|e| AppError::Internal(format!("groups aggregation join error: {e}")))?;
        }
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
    use crate::infrastructure::service_context::{
        ContextType, ServiceContext, ServiceContextConfig,
    };
    use crate::infrastructure::LocalFsProvider;
    use crate::services::ProjectService as _;
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
            home_dir: Some(PathBuf::new()),
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
        assert!(
            matches!(result, Err(AppError::Internal(msg)) if msg.contains("No active ServiceContext"))
        );
    }

    /// 迁移自原 SessionService::get_sessions 同名测试。
    /// SessionService::get_sessions 已删除（与 ProjectService::list_sessions 重叠），
    /// 此测试保留"无 active context 时正确报错"的回归保护。
    #[tokio::test]
    async fn test_list_sessions_returns_error_when_no_active_context() {
        let svc = ProjectServiceImpl::new(make_empty_context_manager());
        let result = svc.list_sessions("any-project").await;
        assert!(
            matches!(result, Err(AppError::Internal(msg)) if msg.contains("No active ServiceContext"))
        );
    }
}
