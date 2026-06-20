//! Search Service — 会话全文搜索与 ID 查找。
//!
//! 持有 `Arc<RwLock<ContextManager>>`，每次方法调用从 active ServiceContext
//! 取 session_searcher（Arc<Mutex<SessionSearcher>>）。SessionSearcher 自身
//! 在 context 内部共享，context 切换时自动替换。

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::error::AppError;
use crate::infrastructure::{ContextManager, FsProvider};
use crate::types::domain::{
    FindSessionByIdResult, FindSessionsByPartialIdResult, SearchSessionsResult,
};

/// 搜索服务 — 会话搜索与 ID 定位（具体实现）。
pub struct SearchServiceImpl {
    context_manager: Arc<RwLock<ContextManager>>,
}

impl SearchServiceImpl {
    /// 创建新的 SearchService。
    pub fn new(context_manager: Arc<RwLock<ContextManager>>) -> Self {
        Self { context_manager }
    }

    /// 取 active ServiceContext 的 searcher Arc。
    async fn searcher(
        &self,
    ) -> Result<std::sync::Arc<tokio::sync::Mutex<crate::discovery::SessionSearcher>>, AppError>
    {
        let active_arc = {
            let mgr = self.context_manager.read().await;
            mgr.get_active()
                .ok_or_else(|| AppError::Internal("No active ServiceContext".into()))?
        };
        let ctx = active_arc.read().await;
        Ok(ctx.session_searcher.clone())
    }
}

#[async_trait]
impl super::search_service_trait::SearchService for SearchServiceImpl {
    async fn search_sessions(
        &self,
        project_id: &str,
        query: &str,
        max_results: u32,
    ) -> Result<SearchSessionsResult, AppError> {
        let max = max_results.min(200).max(1);
        if query.trim().is_empty() {
            return Ok(SearchSessionsResult {
                results: Vec::new(),
                total_matches: 0,
                sessions_searched: 0,
                query: query.to_string(),
                is_partial: None,
            });
        }

        let searcher_arc = self.searcher().await?;
        let mut searcher = searcher_arc.lock().await;
        // ⚠️ SessionSearcher::search_sessions 是 sync fn，不要加 .await。
        Ok(searcher.search_sessions(project_id, query, max))
    }

    async fn search_all_projects(
        &self,
        query: &str,
        max_results: u32,
    ) -> Result<SearchSessionsResult, AppError> {
        let max = max_results.min(200).max(1);
        if query.trim().is_empty() {
            return Ok(SearchSessionsResult {
                results: Vec::new(),
                total_matches: 0,
                sessions_searched: 0,
                query: query.to_string(),
                is_partial: None,
            });
        }

        let searcher_arc = self.searcher().await?;
        let mut searcher = searcher_arc.lock().await;
        Ok(searcher.search_all_projects(query, max))
    }

    async fn find_session_by_id(
        &self,
        session_id: &str,
    ) -> Result<FindSessionByIdResult, AppError> {
        let searcher_arc = self.searcher().await?;
        let mut searcher = searcher_arc.lock().await;
        Ok(searcher.find_session_by_id(session_id))
    }

    async fn find_sessions_by_partial_id(
        &self,
        fragment: &str,
        max_results: usize,
    ) -> Result<FindSessionsByPartialIdResult, AppError> {
        let max = max_results.min(100).max(1);
        if fragment.trim().len() < 3 {
            return Ok(FindSessionsByPartialIdResult {
                found: false,
                results: vec![],
            });
        }

        let searcher_arc = self.searcher().await?;
        let mut searcher = searcher_arc.lock().await;
        Ok(searcher.find_sessions_by_partial_id(fragment.trim(), max))
    }
}

impl super::search_service_trait::SearchServiceRebuild for SearchServiceImpl {
    fn rebuild(
        &self,
        _projects_dir: PathBuf,
        _todos_dir: PathBuf,
        _fs_provider: Arc<dyn FsProvider>,
    ) -> Result<(), AppError> {
        // No-op: SearchServiceImpl 现在从 active ServiceContext 拿 searcher。
        // context 切换（含 claude_root 变更触发的 replace_context）已自动替换 searcher。
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // 引入 trait 使 svc.search_sessions() 方法在作用域内可见
    use crate::services::SearchService as _;
    use std::path::PathBuf;

    /// 构造空 ContextManager（无任何 context 注册），用于测试无 active context 场景。
    fn make_empty_context_manager() -> Arc<RwLock<ContextManager>> {
        Arc::new(RwLock::new(ContextManager::new()))
    }

    /// 构造含 local context 的 ContextManager，用于 ServiceImpl 测试。
    #[allow(dead_code)]
    async fn make_context_manager(
        projects_dir: PathBuf,
        todos_dir: PathBuf,
    ) -> Arc<RwLock<ContextManager>> {
        use crate::infrastructure::service_context::{
            ContextType, ServiceContext, ServiceContextConfig,
        };
        use crate::infrastructure::LocalFsProvider;
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

    #[tokio::test]
    async fn test_search_sessions_returns_error_when_no_active_context() {
        let svc = SearchServiceImpl::new(make_empty_context_manager());
        let result = svc.search_sessions("proj", "query", 10).await;
        assert!(
            matches!(result, Err(AppError::Internal(msg)) if msg.contains("No active ServiceContext"))
        );
    }
}
