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

/// claude 扫描单飞锁（async）：并发调用（getProjects + getRepositoryGroups
/// 同时打进来）只有一个真扫，其余等锁后复用缓存。double-checked：
/// 等到锁后重查缓存，避免重复付整棵树的 SFTP 成本。
static PROJECTS_SCAN_FLIGHT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static SESSIONS_SCAN_FLIGHT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
        let started = std::time::Instant::now();
        let (fs, projects_dir, home) = self.context_deps().await?;
        // claude 纯结果 TTL 缓存（file-change 事件失效，见 listing_cache 模块
        // 文档）：SSH 高延迟下每次全量重扫是侧栏变慢的元凶
        let claude_projects = match crate::infrastructure::listing_cache::get_projects(&projects_dir)
        {
            Some(p) => {
                log::info!(
                    "[perf] scan_projects cache HIT ({} projects) in {:?}",
                    p.len(),
                    started.elapsed()
                );
                p
            }
            None => {
                // 单飞：并发调用方等锁后重查缓存（double-checked），只有一个真扫
                let _guard = PROJECTS_SCAN_FLIGHT.lock().await;
                if let Some(p) =
                    crate::infrastructure::listing_cache::get_projects(&projects_dir)
                {
                    log::info!(
                        "[perf] scan_projects cache HIT after flight ({} projects) in {:?}",
                        p.len(),
                        started.elapsed()
                    );
                    p
                } else {
                    log::info!("[perf] scan_projects START (full scan, projects_dir={})", projects_dir.display());
                    let scanner = self.scanner().await?;
                    let t_scan = std::time::Instant::now();
                    let scanned = scanner.scan_async().await;
                    log::info!(
                        "[perf] scan_projects END: {} projects in {:?} (scanner total)",
                        scanned.len(),
                        t_scan.elapsed()
                    );
                    crate::infrastructure::listing_cache::set_projects(&projects_dir, &scanned);
                    scanned
                }
            }
        };
        // 多 agent 归并在 claude 纯结果上每次内存合成（extra 部分由 agents
        // 聚合缓存挡 IO，两层缓存互不嵌套）
        let merged = tokio::task::spawn_blocking(move || {
            crate::agents::merge_extra_projects(claude_projects, fs.as_ref(), &home)
        })
        .await
        .map_err(|e| AppError::Internal(format!("scan aggregation join error: {e}")))?;
        Ok(merged)
    }

    async fn list_sessions(&self, project_id: &str) -> Result<Vec<Session>, AppError> {
        let started = std::time::Instant::now();
        let (fs, projects_dir, home) = self.context_deps().await?;
        let claude_sessions =
            match crate::infrastructure::listing_cache::get_sessions(&projects_dir, project_id) {
                Some(s) => {
                    log::info!(
                        "[perf] list_sessions({}) cache HIT ({} sessions) in {:?}",
                        project_id,
                        s.len(),
                        started.elapsed()
                    );
                    s
                }
                None => {
                    // 单飞：同项目并发请求（分页首批 + 侧栏刷新）只有一个真扫
                    let _guard = SESSIONS_SCAN_FLIGHT.lock().await;
                    if let Some(s) = crate::infrastructure::listing_cache::get_sessions(
                        &projects_dir,
                        project_id,
                    ) {
                        log::info!(
                            "[perf] list_sessions({}) cache HIT after flight ({} sessions) in {:?}",
                            project_id,
                            s.len(),
                            started.elapsed()
                        );
                        s
                    } else {
                        log::info!("[perf] list_sessions({}) START (full scan)", project_id);
                        let scanner = self.scanner().await?;
                        let t_scan = std::time::Instant::now();
                        let scanned = scanner.list_sessions_async(project_id).await;
                        log::info!(
                            "[perf] list_sessions({}) END: {} sessions in {:?} (scanner total)",
                            project_id,
                            scanned.len(),
                            t_scan.elapsed()
                        );
                        crate::infrastructure::listing_cache::set_sessions(
                            &projects_dir,
                            project_id,
                            &scanned,
                        );
                        scanned
                    }
                }
            };
        let project_id_owned = project_id.to_string();
        let appended = tokio::task::spawn_blocking(move || {
            let t_agg = std::time::Instant::now();
            let result = crate::agents::append_extra_sessions(
                claude_sessions,
                fs.as_ref(),
                &projects_dir,
                &home,
                &project_id_owned,
            );
            log::info!(
                "[perf] append_extra_sessions({}): {} sessions out in {:?} (multi-agent merge)",
                project_id_owned,
                result.len(),
                t_agg.elapsed()
            );
            result
        })
        .await
        .map_err(|e| AppError::Internal(format!("list aggregation join error: {e}")))?;
        Ok(appended)
    }

    async fn get_repository_groups(&self) -> Result<Vec<RepositoryGroup>, AppError> {
        let started = std::time::Instant::now();
        let active_arc = {
            let mgr = self.context_manager.read().await;
            mgr.get_active()
                .ok_or_else(|| AppError::Internal("No active ServiceContext".into()))?
        };
        let projects_dir = {
            let ctx = active_arc.read().await;
            ctx.projects_dir.clone()
        };

        // 复用 scan_projects（含 listing 缓存 + [perf] 日志）获取 claude 项目。
        // 曾经直接调 scanner.scan_async：与并发的 getProjects 各自全量扫描
        //（缓存写入前的窗口内重复付整棵树的 SFTP 成本，且无日志可见 ——
        // 2026-08 SSH 实测「scan_projects END 后仍 loading 很久」的主因之一）。
        let mut projects = self.scan_projects().await?;
        log::info!(
            "[perf] get_repository_groups: {} projects (via scan_projects) in {:?} total",
            projects.len(),
            started.elapsed()
        );
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
