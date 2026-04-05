//! Project Service — 项目扫描、会话列表、仓库分组。
//!
//! 封装 ProjectScanner 和 WorktreeGrouper 的使用，为 commands 和 routes
//! 提供统一的项目数据访问接口。

use std::path::PathBuf;
use std::sync::Arc;

use crate::discovery::{ProjectScanner, WorktreeGrouper};
use crate::infrastructure::fs_provider::{FsProvider, LocalFsProvider};
use crate::types::domain::{Project, RepositoryGroup, Session};

/// 项目服务 — 扫描、列出、分组项目与会话（具体实现）。
pub struct ProjectServiceImpl {
    fs_provider: Arc<dyn FsProvider>,
    projects_dir: PathBuf,
    todos_dir: PathBuf,
}

impl ProjectServiceImpl {
    /// 创建新的 ProjectService。
    pub fn new(
        fs_provider: Arc<dyn FsProvider>,
        projects_dir: PathBuf,
        todos_dir: PathBuf,
    ) -> Self {
        Self {
            fs_provider,
            projects_dir,
            todos_dir,
        }
    }

    /// 创建使用默认 LocalFsProvider 的 ProjectService（便捷构造）。
    #[allow(dead_code)]
    pub fn with_local_fs(projects_dir: PathBuf, todos_dir: PathBuf) -> Self {
        Self::new(Arc::new(LocalFsProvider::new()), projects_dir, todos_dir)
    }

    // ─── 内部 Scanner 访问 ───

    fn scanner(&self) -> ProjectScanner {
        ProjectScanner::with_paths(
            self.projects_dir.clone(),
            self.todos_dir.clone(),
            self.fs_provider.clone(),
        )
    }

    // ─── 公共方法 ───

    /// 扫描所有项目（get_projects 命令）。
    pub fn scan_projects(&self) -> Vec<Project> {
        self.scanner().scan()
    }

    /// 列出指定项目的所有会话元数据。
    ///
    /// 被 SessionService 内部复用，也供 get_worktree_sessions 命令直接调用。
    pub fn list_sessions(&self, project_id: &str) -> Vec<Session> {
        self.scanner().list_sessions(project_id)
    }

    /// 获取按 git 仓库分组的项目列表（get_repository_groups 命令）。
    pub fn get_repository_groups(&self) -> Vec<RepositoryGroup> {
        if !self.projects_dir.exists() {
            return Vec::new();
        }

        let projects = self.scan_projects();
        if projects.is_empty() {
            return Vec::new();
        }

        let grouper = WorktreeGrouper::new(self.projects_dir.clone());
        grouper.group_by_repository(projects)
    }

    /// 获取指定 worktree 的会话列表（get_worktree_sessions 命令）。
    pub fn get_worktree_sessions(&self, worktree_id: &str) -> Vec<Session> {
        self.list_sessions(worktree_id)
    }
}

// ════════════════════════════════════════════════════════════════
//  Trait Implementation
// ════════════════════════════════════════════════════════════════

impl super::project_service_trait::ProjectService for ProjectServiceImpl {
    fn scan_projects(&self) -> Vec<crate::types::domain::Project> {
        self.scan_projects()
    }

    fn list_sessions(&self, project_id: &str) -> Vec<crate::types::domain::Session> {
        self.list_sessions(project_id)
    }

    fn get_repository_groups(&self) -> Vec<crate::types::domain::RepositoryGroup> {
        self.get_repository_groups()
    }

    fn get_worktree_sessions(&self, worktree_id: &str) -> Vec<crate::types::domain::Session> {
        self.get_worktree_sessions(worktree_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_test_dirs(temp: &tempfile::TempDir) -> (PathBuf, PathBuf) {
        let projects = temp.path().join("projects");
        let todos = temp.path().join("todos");
        fs::create_dir_all(&projects).unwrap();
        fs::create_dir_all(&todos).unwrap();
        (projects, todos)
    }

    #[test]
    fn test_scan_projects_empty() {
        let dir = tempfile::tempdir().unwrap();
        let (projects, todos) = setup_test_dirs(&dir);
        let svc = ProjectServiceImpl::with_local_fs(projects, todos);
        assert!(svc.scan_projects().is_empty());
    }

    #[test]
    fn test_list_sessions_empty_project() {
        let dir = tempfile::tempdir().unwrap();
        let (projects, todos) = setup_test_dirs(&dir);
        let svc = ProjectServiceImpl::with_local_fs(projects, todos);
        assert!(svc.list_sessions("nonexistent").is_empty());
    }

    #[test]
    fn test_get_repository_groups_empty() {
        let dir = tempfile::tempdir().unwrap();
        let (projects, todos) = setup_test_dirs(&dir);
        let svc = ProjectServiceImpl::with_local_fs(projects, todos);
        assert!(svc.get_repository_groups().is_empty());
    }

    #[test]
    fn test_get_worktree_sessions_empty() {
        let dir = tempfile::tempdir().unwrap();
        let (projects, todos) = setup_test_dirs(&dir);
        let svc = ProjectServiceImpl::with_local_fs(projects, todos);
        assert!(svc.get_worktree_sessions("abc").is_empty());
    }
}
