//! 服务上下文 — 封装单个工作空间的所有会话数据服务。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::discovery::{ProjectScanner, SessionSearcher, SubagentResolver};
use crate::infrastructure::DataCache;
use crate::infrastructure::file_watcher::FileWatcher;
use crate::infrastructure::fs_provider::FsProvider;

/// 服务上下文配置。
#[derive(Clone)]
pub struct ServiceContextConfig {
    pub id: String,
    pub context_type: ContextType,
    pub projects_dir: PathBuf,
    #[allow(dead_code)]
    pub todos_dir: PathBuf,
    pub fs_provider: Arc<dyn FsProvider>,
    /// 可选的共享缓存。若提供，与 AppState 共享同一缓存实例，
    /// 确保文件监听器的缓存失效能被 IPC 命令感知。
    pub cache: Option<DataCache>,
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
    pub fs_provider: Arc<dyn FsProvider>,
    pub cache: DataCache,
    #[allow(dead_code)]
    pub project_scanner: ProjectScanner,
    #[allow(dead_code)]
    pub subagent_resolver: SubagentResolver,
    #[allow(dead_code)]
    pub session_searcher: Arc<Mutex<SessionSearcher>>,
    pub file_watcher: Arc<Mutex<FileWatcher>>,
    pub todo_watcher: Arc<Mutex<FileWatcher>>,
    /// 当前 watcher 使用的取消令牌。每次 spawn 创建新令牌，stop 时取消。
    /// 使用 `RwLock<Option<...>>` 使 token 可替换，支持可逆的 stop/start 生命周期。
    pub watcher_cancel_token: RwLock<Option<CancellationToken>>,
    pub is_started: AtomicBool,
}

impl ServiceContext {
    pub fn new(config: ServiceContextConfig) -> Self {
        let project_scanner = ProjectScanner::with_paths(
            config.projects_dir.clone(),
            config.todos_dir.clone(),
            config.fs_provider.clone(),
        );
        let session_searcher = Arc::new(Mutex::new(
            SessionSearcher::new(config.projects_dir.clone(), config.todos_dir.clone(), config.fs_provider.clone(), None),
        ));
        let subagent_resolver = SubagentResolver::new(config.projects_dir.clone(), config.fs_provider.clone());
        let cache = config.cache.unwrap_or_else(|| {
            // 与 Electron 对齐：支持 CLAUDE_CONTEXT_DISABLE_CACHE 环境变量禁用缓存
            if std::env::var("CLAUDE_CONTEXT_DISABLE_CACHE").map(|v| v == "1").unwrap_or(false) {
                DataCache::disabled()
            } else {
                DataCache::new()
            }
        });
        let file_watcher = Arc::new(Mutex::new(FileWatcher::new(config.fs_provider.clone())));
        let todo_watcher = Arc::new(Mutex::new(FileWatcher::new(config.fs_provider.clone())));

        Self {
            id: config.id,
            context_type: config.context_type,
            projects_dir: config.projects_dir,
            todos_dir: config.todos_dir,
            fs_provider: config.fs_provider,
            cache,
            project_scanner,
            subagent_resolver,
            session_searcher,
            file_watcher,
            todo_watcher,
            watcher_cancel_token: RwLock::new(None),
            is_started: AtomicBool::new(false),
        }
    }

    /// 启动文件监听器任务。
    ///
    /// 委托给 WatcherOrchestrator 执行三个并发 watcher 任务的 spawn。
    pub async fn spawn_watcher_tasks(
        &self,
        app_handle: tauri::AppHandle,
        config_manager: Arc<crate::infrastructure::ConfigManager>,
        notification_manager: Arc<tokio::sync::RwLock<crate::infrastructure::NotificationManager>>,
    ) {
        use crate::infrastructure::watcher_orchestrator::WatcherOrchestrator;

        let orchestrator = WatcherOrchestrator::new(
            self.projects_dir.clone(),
            self.todos_dir.clone(),
            self.fs_provider.clone(),
            self.cache.clone(),
            self.file_watcher.clone(),
            self.todo_watcher.clone(),
        );

        let cancel_token = orchestrator.spawn_all(
            app_handle,
            config_manager,
            notification_manager,
        ).await;

        // 存储 cancel token 以支持 stop_watcher_tasks
        {
            let mut guard = self.watcher_cancel_token.write().await;
            if let Some(old) = guard.take() {
                old.cancel();
            }
            *guard = Some(cancel_token);
        }

        self.is_started.store(true, Ordering::Relaxed);
        log::info!(
            "ServiceContext '{}': watcher tasks spawned (projects={}, todos={})",
            self.id,
            self.projects_dir.display(),
            self.todos_dir.display(),
        );
    }

    /// 停止所有文件监听器任务。
    ///
    /// 取消当前 token 并清除引用。后续 `spawn_watcher_tasks` 会创建新 token，
    /// 确保可重复的 stop/start 生命周期（与 Electron 行为对齐）。
    pub async fn stop_watcher_tasks(&self) {
        let mut guard = self.watcher_cancel_token.write().await;
        if let Some(token) = guard.take() {
            token.cancel();
            log::info!("ServiceContext '{}': watcher tasks cancelled", self.id);
        }
    }
}
