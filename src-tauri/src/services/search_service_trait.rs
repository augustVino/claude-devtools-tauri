//! Search Service Trait — 会话全文搜索与 ID 查找的抽象接口。

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use crate::error::AppError;
use crate::infrastructure::fs_provider::FsProvider;
use crate::types::domain::{
    FindSessionByIdResult, FindSessionsByPartialIdResult, SearchSessionsResult,
};

/// 同步重建接口 — 用于在 claude root path 变更后重建内部搜索索引。
pub trait SearchServiceRebuild: Send + Sync {
    fn rebuild(&self, projects_dir: PathBuf, todos_dir: PathBuf, fs_provider: Arc<dyn FsProvider>) -> Result<(), AppError>;
}

#[async_trait]
pub trait SearchService: Send + Sync {
    async fn search_sessions(&self, project_id: &str, query: &str, max_results: u32) -> Result<SearchSessionsResult, AppError>;
    async fn search_all_projects(&self, query: &str, max_results: u32) -> Result<SearchSessionsResult, AppError>;
    async fn find_session_by_id(&self, session_id: &str) -> Result<FindSessionByIdResult, AppError>;
    async fn find_sessions_by_partial_id(&self, fragment: &str, max_results: usize) -> Result<FindSessionsByPartialIdResult, AppError>;
}

/// Compound trait combining SearchService (async queries) and SearchServiceRebuild (sync rebuild).
///
/// Rust trait objects cannot use `dyn A + dyn B` when both are non-auto traits.
/// This supertrait is the canonical way to hold a reference that needs both capabilities.
pub trait SearchServiceFull: SearchService + SearchServiceRebuild {}
// Blanket impl: any type that implements both automatically implements Full
impl<T: SearchService + SearchServiceRebuild> SearchServiceFull for T {}
