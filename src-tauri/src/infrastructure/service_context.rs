//! 服务上下文 — 封装单个工作空间的所有会话数据服务。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::discovery::{ProjectScanner, SessionSearcher, SubagentResolver};
use crate::infrastructure::file_watcher::FileWatcher;
use crate::infrastructure::fs_provider::FsProvider;
use crate::infrastructure::DataCache;

/// 服务上下文配置。
#[derive(Clone)]
pub struct ServiceContextConfig {
    pub id: String,
    pub context_type: ContextType,
    pub projects_dir: PathBuf,
    /// 显式指定上下文 home（多 agent 聚合的数据根推导基准）。
    /// None → 按默认推导（Local=真实用户 home；Ssh=projects_dir 标准布局
    /// 反推）。生产路径不传；测试传 tempdir 隔离真实 home 的 ~/.pi 等数据。
    pub home_dir: Option<PathBuf>,
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
    /// 当前上下文的 home 目录（多 agent 聚合用：推导 ~/.pi 等数据根）。
    /// Local：真实用户 home（不受 claude_root_path 自定义影响）；
    /// Ssh：仅当 projects_dir 呈 `{home}/.claude/projects` 标准布局时可推导，
    /// 否则为空 PathBuf —— 聚合层降级为无额外 agent（debug 日志可排查）。
    pub home_dir: PathBuf,
    pub todos_dir: PathBuf,
    pub fs_provider: Arc<dyn FsProvider>,
    pub cache: DataCache,
    #[allow(dead_code)]
    pub project_scanner: ProjectScanner,
    #[allow(dead_code)]
    pub subagent_resolver: SubagentResolver,
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
        // home 推导：Local 用真实用户 home（不受 claude_root_path 自定义影响，
        // 修复自定义根下 ~/.pi 探测落空）；Ssh 仅标准布局可推导，否则空
        // （聚合层降级为无额外 agent）
        let home_dir = config.home_dir.unwrap_or_else(|| match config.context_type {
            ContextType::Local => dirs::home_dir().unwrap_or_default(),
            ContextType::Ssh => {
                let inferred = config
                    .projects_dir
                    .parent()
                    .and_then(|p| p.parent())
                    .map(|p| p.to_path_buf())
                    .unwrap_or_default();
                if config.projects_dir.ends_with(".claude/projects") {
                    inferred
                } else {
                    log::warn!(
                        "ServiceContext: non-standard ssh projects_dir {} — extra-agent aggregation disabled",
                        config.projects_dir.display()
                    );
                    PathBuf::new()
                }
            }
        });
        let session_searcher = Arc::new(Mutex::new(SessionSearcher::new(
            config.projects_dir.clone(),
            config.todos_dir.clone(),
            config.fs_provider.clone(),
            home_dir.clone(),
            None,
        )));
        let subagent_resolver =
            SubagentResolver::new(config.projects_dir.clone(), config.fs_provider.clone());
        let cache = config.cache.unwrap_or_else(|| {
            // 与 Electron 对齐：支持 CLAUDE_CONTEXT_DISABLE_CACHE 环境变量禁用缓存
            if std::env::var("CLAUDE_CONTEXT_DISABLE_CACHE")
                .map(|v| v == "1")
                .unwrap_or(false)
            {
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
            home_dir,
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

        let mut orchestrator = WatcherOrchestrator::new(
            self.projects_dir.clone(),
            self.todos_dir.clone(),
            self.fs_provider.clone(),
            self.cache.clone(),
            self.file_watcher.clone(),
            self.todo_watcher.clone(),
        );

        // Wire project-level cache invalidation callback (mirrors Electron's
        // FileWatcher→ProjectScanner.invalidateCachesForProject linkage).
        // DataCache.invalidate_project is async; spawn a lightweight task to avoid
        // blocking the file watcher event loop.
        let cache_for_invalidation = self.cache.clone();
        orchestrator.set_on_cache_invalidate(move |project_id: &str| {
            let cache = cache_for_invalidation.clone();
            let pid = project_id.to_string();
            tauri::async_runtime::spawn(async move {
                cache.invalidate_project(&pid).await;
            });
        });

        let cancel_token = orchestrator
            .spawn_all(app_handle, config_manager, notification_manager)
            .await;

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
        {
            let mut guard = self.watcher_cancel_token.write().await;
            if let Some(token) = guard.take() {
                token.cancel();
                log::info!("ServiceContext '{}': watcher tasks cancelled", self.id);
            }
        }
        // 主动 stop FileWatcher，确保 is_watching 在新任务 watch 前被重置。
        //
        // 必要性：旧监听任务从收到 cancel 到自行执行 stop() 是异步的（select
        // 唤醒 → 抢 FileWatcher 锁 → stop）。快速切换（local→ssh→local）时，
        // 新任务可能抢先 watch，触发 watch_local 的 is_watching 防重入检查返回
        // "Already watching" 而夭折；随后旧任务 stop 退出 → 监听真空 → 会话详情
        // 不再更新。这里同步 stop 保证 is_watching 在 spawn 新任务前已被清理。
        //
        // 与 spawn_all 协作：主/todo 监听任务退出时不再调用 stop()，FileWatcher
        // 清理统一由本方法负责（stop() 幂等，可安全多次调用）。
        self.file_watcher.lock().await.stop().await;
        self.todo_watcher.lock().await.stop().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::fs_provider::LocalFsProvider;

    fn make_local_context(dir: &std::path::Path) -> ServiceContext {
        ServiceContext::new(ServiceContextConfig {
            id: "test".to_string(),
            context_type: ContextType::Local,
            home_dir: Some(PathBuf::new()),
            projects_dir: dir.to_path_buf(),
            todos_dir: dir.join("todos"),
            fs_provider: std::sync::Arc::new(LocalFsProvider::new()),
            cache: None,
        })
    }

    /// 复现快速切换（local→ssh→local）竞态的回归测试。
    ///
    /// 主监听任务已 watch（is_watching=true）后，context 切换会调用
    /// `stop_watcher_tasks`。它必须主动重置 `is_watching`，否则切回 local 后
    /// 新任务的 `watch_local` 会因防重入返回 "Already watching" 而夭折，
    /// 导致 local 监听永久死亡、会话详情不再更新。
    #[tokio::test]
    async fn stop_watcher_tasks_resets_is_watching_for_rewatch() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = make_local_context(tmp.path());

        // 模拟主监听任务已执行 watch（is_watching=true）
        {
            let mut fw = ctx.file_watcher.lock().await;
            fw.watch(tmp.path()).await.unwrap();
            assert!(fw.is_watching().await);
        }

        // context 切换时的 stop：必须重置 is_watching
        ctx.stop_watcher_tasks().await;
        {
            let fw = ctx.file_watcher.lock().await;
            assert!(
                !fw.is_watching().await,
                "stop_watcher_tasks 必须重置 is_watching，否则快速切换后新任务 watch 会夭折"
            );
        }

        // 切回后新任务 watch：应成功（不再 "Already watching"）
        {
            let mut fw = ctx.file_watcher.lock().await;
            fw.watch(tmp.path()).await.expect(
                "stop_watcher_tasks 后必须能重新 watch —— local→ssh→local 不丢失监听的前提",
            );
        }
    }
}
