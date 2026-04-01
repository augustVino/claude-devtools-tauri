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
use tauri::Manager;

/// 服务上下文配置。
#[derive(Clone)]
pub struct ServiceContextConfig {
    pub id: String,
    pub context_type: ContextType,
    pub projects_dir: PathBuf,
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
    pub project_scanner: ProjectScanner,
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
    /// 生成三个并发 tokio 任务：
    /// 1. 主监听器（JSONL/JSON 变更 → 事件发射 + SSE 广播）
    /// 2. 错误检测管道（JSONL 变更 → 错误检测 → 通知）
    /// 3. Todo 监听器（Todo 文件变更 → 事件发射 + SSE 广播）
    ///
    /// 所有任务在 `watcher_cancel_token` 取消时退出。
    pub async fn spawn_watcher_tasks(
        &self,
        app_handle: tauri::AppHandle,
        config_manager: Arc<crate::infrastructure::ConfigManager>,
        notification_manager: Arc<tokio::sync::RwLock<crate::infrastructure::NotificationManager>>,
    ) {
        // 每次启动时创建新令牌，确保 stop/start 可逆（与 Electron 的 isWatching 标志对齐）
        let cancel_token = CancellationToken::new();
        {
            let mut guard = self.watcher_cancel_token.write().await;
            // 如果有旧 token 未取消，先取消
            if let Some(old) = guard.take() {
                old.cancel();
            }
            *guard = Some(cancel_token.clone());
        }
        let projects_dir = self.projects_dir.clone();
        let todos_dir = self.todos_dir.clone();
        let file_watcher = self.file_watcher.clone();
        let file_watcher_for_error = self.file_watcher.clone();
        let todo_watcher = self.todo_watcher.clone();

        // === 主文件监听器任务 ===
        {
            let cancel = cancel_token.clone();
            let app = app_handle.clone();
            let projects_dir = projects_dir.clone();
            let cache = self.cache.clone();
            let fs_provider = self.fs_provider.clone();

            tauri::async_runtime::spawn(async move {
                let mut watcher = file_watcher.lock().await;
                if !fs_provider.exists(&projects_dir).unwrap_or(false) {
                    if let Err(e) = tokio::fs::create_dir_all(&projects_dir).await {
                        log::error!("Failed to create projects directory: {}", e);
                        return;
                    }
                }
                if let Err(e) = watcher.watch(&projects_dir).await {
                    log::error!("Failed to start main FileWatcher: {}", e);
                    return;
                }
                drop(watcher);

                let mut receiver = { file_watcher.lock().await.receiver() };
                loop {
                    tokio::select! {
                        result = receiver.recv() => {
                            match result {
                                Ok(event) => {
                                    // 与 Electron 行为一致：文件变化时立即失效缓存，
                                    // 确保后续 getSessionDetail 调用重新解析 JSONL 文件
                                    if let (Some(pid), Some(sid)) =
                                        (&event.project_id, &event.session_id)
                                    {
                                        cache.invalidate_session(pid, sid).await;
                                    }

                                    crate::events::emit_file_change(&app, event.clone());
                                    if let Some(broadcaster) =
                                        app.try_state::<crate::http::sse::SSEBroadcaster>()
                                    {
                                        let _ = broadcaster.inner().send(
                                            crate::http::sse::BackendEvent::FileChange(event),
                                        );
                                    }
                                }
                                Err(_) => {
                                    log::info!("Main FileWatcher receiver closed");
                                    break;
                                }
                            }
                        }
                        _ = cancel.cancelled() => {
                            log::info!("Main FileWatcher cancelled for context");
                            break;
                        }
                    }
                }
                file_watcher.lock().await.stop().await;
            });
        }

        // === 错误检测管道任务 ===
        // 共享主 file_watcher 的 broadcast receiver，不创建独立 watcher
        {
            let cancel = cancel_token.clone();
            let app = app_handle.clone();

            tauri::async_runtime::spawn(async move {
                // 订阅主 watcher 的事件
                let mut error_rx = { file_watcher_for_error.lock().await.receiver() };
                let detector = crate::error::error_detector::ErrorDetector::new(config_manager);
                loop {
                    tokio::select! {
                        result = error_rx.recv() => {
                            match result {
                                Ok(event) => {
                                    let path = std::path::Path::new(&event.path);
                                    if path.extension().map(|e| e != "jsonl").unwrap_or(true) {
                                        continue;
                                    }
                                    if crate::utils::is_subagent_file(&event.path) {
                                        continue;
                                    }
                                    let session_id = path.file_stem()
                                        .map(|s| s.to_string_lossy().to_string())
                                        .unwrap_or_default();
                                    let project_id = event.project_id.clone().unwrap_or_default();
                                    let messages = crate::parsing::jsonl_parser::parse_jsonl_file(path).await;
                                    if messages.is_empty() {
                                        continue;
                                    }
                                    let errors = detector.detect_errors(
                                        &messages, &session_id, &project_id, &event.path,
                                    ).await;
                                    let mgr = notification_manager.read().await;
                                    for detected_error in errors {
                                        let _ = mgr.add_error(detected_error).await;
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                        _ = cancel.cancelled() => {
                            log::info!("Error detection pipeline cancelled for context");
                            break;
                        }
                    }
                }
            });
        }

        // === Todo 文件监听器任务 ===
        {
            let cancel = cancel_token.clone();
            let app = app_handle;
            let todo_fs_provider = self.fs_provider.clone();

            tauri::async_runtime::spawn(async move {
                let mut todo_watcher_guard = todo_watcher.lock().await;
                if !todo_fs_provider.exists(&todos_dir).unwrap_or(false) {
                    if let Err(e) = tokio::fs::create_dir_all(&todos_dir).await {
                        log::error!("Failed to create todos directory: {}", e);
                        return;
                    }
                }
                if let Err(e) = todo_watcher_guard.watch(&todos_dir).await {
                    log::error!("Failed to start todo FileWatcher: {}", e);
                    return;
                }
                drop(todo_watcher_guard);
                let mut receiver = { todo_watcher.lock().await.receiver() };
                loop {
                    tokio::select! {
                        result = receiver.recv() => {
                            match result {
                                Ok(event) => {
                                    let session_id = std::path::Path::new(&event.path)
                                        .file_stem()
                                        .map(|s| s.to_string_lossy().to_string())
                                        .unwrap_or_default();
                                    let todo_event = crate::events::TodoChangeEvent {
                                        session_id: session_id.clone(),
                                    };
                                    crate::events::emit_todo_change(&app, todo_event.clone());
                                    if let Some(broadcaster) =
                                        app.try_state::<crate::http::sse::SSEBroadcaster>()
                                    {
                                        let _ = broadcaster.inner().send(
                                            crate::http::sse::BackendEvent::TodoChange(todo_event),
                                        );
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                        _ = cancel.cancelled() => {
                            log::info!("Todo FileWatcher cancelled for context");
                            break;
                        }
                    }
                }
                todo_watcher.lock().await.stop().await;
            });
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
