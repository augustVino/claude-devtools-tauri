//! 服务上下文 — 封装单个工作空间的所有会话数据服务。

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::discovery::{ProjectScanner, SessionSearcher, SubagentResolver};
use crate::infrastructure::DataCache;
use crate::infrastructure::file_watcher::FileWatcher;

/// 服务上下文配置。
#[derive(Debug, Clone)]
pub struct ServiceContextConfig {
    pub id: String,
    pub context_type: ContextType,
    pub projects_dir: PathBuf,
    pub todos_dir: PathBuf,
}

/// 上下文类型。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextType {
    Local,
    Ssh,
}

/// 服务上下文 — 封装单个工作空间的完整服务栈。
pub struct ServiceContext {
    pub id: String,
    pub context_type: ContextType,
    pub projects_dir: PathBuf,
    pub todos_dir: PathBuf,
    pub cache: DataCache,
    pub project_scanner: ProjectScanner,
    pub subagent_resolver: SubagentResolver,
    pub session_searcher: Arc<Mutex<SessionSearcher>>,
    pub file_watcher: Arc<Mutex<FileWatcher>>,
    pub todo_watcher: Arc<Mutex<FileWatcher>>,
    pub watcher_cancel_token: CancellationToken,
    pub is_started: bool,
}

impl ServiceContext {
    pub fn new(config: ServiceContextConfig) -> Self {
        let project_scanner = ProjectScanner::with_paths(
            config.projects_dir.clone(),
            config.todos_dir.clone(),
        );
        let session_searcher = Arc::new(Mutex::new(
            SessionSearcher::new(config.projects_dir.clone()),
        ));
        let subagent_resolver = SubagentResolver::new(config.projects_dir.clone());
        let cache = DataCache::new();
        let file_watcher = Arc::new(Mutex::new(FileWatcher::new()));
        let todo_watcher = Arc::new(Mutex::new(FileWatcher::new()));

        Self {
            id: config.id,
            context_type: config.context_type,
            projects_dir: config.projects_dir,
            todos_dir: config.todos_dir,
            cache,
            project_scanner,
            subagent_resolver,
            session_searcher,
            file_watcher,
            todo_watcher,
            watcher_cancel_token: CancellationToken::new(),
            is_started: false,
        }
    }
}
