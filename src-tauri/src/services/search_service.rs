//! Search Service — 会话全文搜索与 ID 查找。
//!
//! 封装 SessionSearcher 的使用，为 commands 和 routes
//! 提供统一的搜索接口。内部使用 spawn_blocking 避免阻塞 async runtime。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::discovery::SessionSearcher;
use crate::infrastructure::fs_provider::{FsProvider, LocalFsProvider};
use crate::types::domain::{
    FindSessionsByPartialIdResult, FindSessionByIdResult, SearchSessionsResult,
};

/// 搜索服务 — 会话搜索与 ID 定位。
pub struct SearchService {
    searcher: Arc<Mutex<SessionSearcher>>,
}

impl SearchService {
    /// 创建新的 SearchService。
    pub fn new(
        projects_dir: PathBuf,
        todos_dir: PathBuf,
        fs_provider: Arc<dyn FsProvider>,
    ) -> Self {
        Self {
            searcher: Arc::new(Mutex::new(SessionSearcher::new(
                projects_dir,
                todos_dir,
                fs_provider,
                None,
            ))),
        }
    }

    /// 创建使用默认 LocalFsProvider 的 SearchService（便捷构造）。
    #[allow(dead_code)]
    pub fn with_local_fs(projects_dir: PathBuf, todos_dir: PathBuf) -> Self {
        Self::new(projects_dir, todos_dir, Arc::new(LocalFsProvider::new()))
    }

    // ─── 公共方法 ───

    /// 在指定项目中搜索会话（search_sessions 命令）。
    pub async fn search_sessions(
        &self,
        project_id: &str,
        query: &str,
        max_results: u32,
    ) -> Result<SearchSessionsResult, String> {
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

        let searcher = self.searcher.clone();
        let pid = project_id.to_string();
        let q = query.to_string();

        tokio::task::spawn_blocking(move || -> Result<SearchSessionsResult, String> {
            let mut s = searcher.lock().map_err(|e| e.to_string())?;
            Ok(s.search_sessions(&pid, &q, max))
        })
        .await
        .map_err(|e| format!("search task panicked: {}", e))?
    }

    /// 跨所有项目搜索会话（search_all_projects 命令）。
    pub async fn search_all_projects(
        &self,
        query: &str,
        max_results: u32,
    ) -> Result<SearchSessionsResult, String> {
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

        let searcher = self.searcher.clone();
        let q = query.to_string();

        tokio::task::spawn_blocking(move || -> Result<SearchSessionsResult, String> {
            let mut s = searcher.lock().map_err(|e| e.to_string())?;
            Ok(s.search_all_projects(&q, max))
        })
        .await
        .map_err(|e| format!("search_all_projects task panicked: {}", e))?
    }

    /// 按 UUID 精确查找会话（find_session_by_id 命令）。
    pub async fn find_session_by_id(
        &self,
        session_id: &str,
    ) -> Result<FindSessionByIdResult, String> {
        let searcher = self.searcher.clone();
        let sid = session_id.to_string();

        tokio::task::spawn_blocking(move || -> Result<FindSessionByIdResult, String> {
            let mut s = searcher.lock().map_err(|e| e.to_string())?;
            Ok(s.find_session_by_id(&sid))
        })
        .await
        .map_err(|e| format!("find_session_by_id task panicked: {}", e))?
    }

    /// 按部分 ID 模糊匹配查找会话（find_sessions_by_partial_id 命令）。
    pub async fn find_sessions_by_partial_id(
        &self,
        fragment: &str,
        max_results: usize,
    ) -> Result<FindSessionsByPartialIdResult, String> {
        let max = max_results.min(100).max(1);

        if fragment.trim().len() < 3 {
            return Ok(FindSessionsByPartialIdResult {
                found: false,
                results: vec![],
            });
        }

        let searcher = self.searcher.clone();
        let frag = fragment.trim().to_string();

        tokio::task::spawn_blocking(move || -> Result<FindSessionsByPartialIdResult, String> {
            let mut s = searcher.lock().map_err(|e| e.to_string())?;
            Ok(s.find_sessions_by_partial_id(&frag, max))
        })
        .await
        .map_err(|e| format!("find_sessions_by_partial_id task panicked: {}", e))?
    }

    /// Rebuild the internal SessionSearcher with new paths (e.g., after claude root change).
    pub fn rebuild(&self, projects_dir: PathBuf, todos_dir: PathBuf, fs_provider: Arc<dyn FsProvider>) -> Result<(), String> {
        let mut guard = self.searcher.lock().map_err(|e| format!("Failed to lock searcher: {e}"))?;
        *guard = SessionSearcher::new(projects_dir, todos_dir, fs_provider, None);
        Ok(())
    }
}
