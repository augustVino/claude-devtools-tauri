//! 会话数据存储与检索抽象。
//!
//! 将数据访问逻辑从 SessionService 中分离，使 Service 只关注业务编排。
//! 通过 trait object (`Arc<dyn SessionRepository>`) 支持 mock 测试和多实现切换。

use std::path::{Path, PathBuf};
use async_trait::async_trait;

use crate::error::AppError;
use crate::parsing::ParsedSession;
use crate::infrastructure::fs_provider::FsStatResult;

/// 会话文件条目 — 用于批量读取的轻量级描述。
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SessionFileItem {
    pub path: PathBuf,
    pub session_id: String,
    pub mtime_ms: u64,
}

/// 会话删除结果 — 返回清理的关联文件/目录数量。
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DeleteFilesResult {
    /// 被删除的关联条目数（不含主 JSONL 文件本身）
    pub associated_deleted: u32,
}

/// 会话 Repository trait — 抽象会话数据的存储与检索操作。
#[async_trait]
pub trait SessionRepository: Send + Sync {
    /// 从文件系统读取并解析原始会话数据。
    #[allow(dead_code)]
    async fn read_raw_session(
        &self,
        project_id: &str,
        session_file: &str,
    ) -> Result<ParsedSession, AppError>;

    /// 检查指定会话是否存在。
    #[allow(dead_code)]
    async fn session_exists(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<bool, AppError>;

    /// 获取会话文件的元信息（不解析内容）。
    #[allow(dead_code)]
    async fn session_stat(&self, path: &Path) -> Result<FsStatResult, AppError>;

    /// 批量获取项目下所有会话的元数据列表（用于分页/列表场景）。
    #[allow(dead_code)]
    async fn list_session_files(&self, project_id: &str) -> Result<Vec<SessionFileItem>, AppError>;

    /// 删除指定会话及其所有关联文件。返回清理的关联条目数。
    #[allow(dead_code)]
    async fn delete_session_files(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<DeleteFilesResult, AppError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Mock 实现 — 用于单元测试 SessionService 时替代真实文件系统访问
    pub struct MockSessionRepository;

    #[async_trait]
    impl SessionRepository for MockSessionRepository {
        async fn read_raw_session(
            &self, _project_id: &str, _session_file: &str,
        ) -> Result<ParsedSession, AppError> {
            Err(AppError::NotFound("mock: no sessions".into()))
        }

        async fn session_exists(
            &self, _project_id: &str, _session_id: &str,
        ) -> Result<bool, AppError> {
            Ok(false)
        }

        async fn session_stat(&self, _path: &Path) -> Result<FsStatResult, AppError> {
            Err(AppError::NotFound("mock: no stat".into()))
        }

        async fn list_session_files(&self, _project_id: &str) -> Result<Vec<SessionFileItem>, AppError> {
            Ok(Vec::new())
        }

        async fn delete_session_files(
            &self, _project_id: &str, _session_id: &str,
        ) -> Result<DeleteFilesResult, AppError> {
            Ok(DeleteFilesResult { associated_deleted: 0 })
        }
    }

    #[tokio::test]
    async fn test_mock_repository_returns_empty_list() {
        let repo: Arc<dyn SessionRepository> = Arc::new(MockSessionRepository);
        let files = repo.list_session_files("test-project").await.unwrap();
        assert!(files.is_empty());
    }

    #[tokio::test]
    async fn test_mock_repository_delete_returns_zero() {
        let repo: Arc<dyn SessionRepository> = Arc::new(MockSessionRepository);
        let result = repo.delete_session_files("proj", "sess").await.unwrap();
        assert_eq!(result.associated_deleted, 0);
    }

    #[tokio::test]
    async fn test_mock_repository_session_not_exists() {
        let repo: Arc<dyn SessionRepository> = Arc::new(MockSessionRepository);
        let exists = repo.session_exists("proj", "sess").await.unwrap();
        assert!(!exists);
    }
}
