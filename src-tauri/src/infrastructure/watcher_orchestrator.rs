//! Watcher 任务编排器 — 从 ServiceContext 中提取的文件监听任务管理。
//!
//! 负责 spawn 三个并发 tokio task：主监听器、错误检测管道、Todo 监听器。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::infrastructure::{
    ConfigManager, DataCache, FileWatcher, NotificationManager,
    fs_provider::FsProvider,
};
use tauri::Manager;

/// Watcher 编排器 — 管理文件监听任务的启动生命周期。
pub struct WatcherOrchestrator {
    projects_dir: PathBuf,
    todos_dir: PathBuf,
    fs_provider: Arc<dyn FsProvider>,
    cache: DataCache,
    file_watcher: Arc<Mutex<FileWatcher>>,
    todo_watcher: Arc<Mutex<FileWatcher>>,
}

impl WatcherOrchestrator {
    pub fn new(
        projects_dir: PathBuf,
        todos_dir: PathBuf,
        fs_provider: Arc<dyn FsProvider>,
        cache: DataCache,
        file_watcher: Arc<Mutex<FileWatcher>>,
        todo_watcher: Arc<Mutex<FileWatcher>>,
    ) -> Self {
        Self {
            projects_dir,
            todos_dir,
            fs_provider,
            cache,
            file_watcher,
            todo_watcher,
        }
    }

    /// 启动所有 watcher 任务（与原 ServiceContext::spawn_watcher_tasks 逻辑完全一致）。
    ///
    /// 返回 CancellationToken 用于后续取消。
    pub async fn spawn_all(
        &self,
        app_handle: tauri::AppHandle,
        config_manager: Arc<ConfigManager>,
        notification_manager: Arc<RwLock<NotificationManager>>,
    ) -> CancellationToken {
        let cancel_token = CancellationToken::new();

        // === 主文件监听器任务 ===
        {
            let cancel = cancel_token.clone();
            let app = app_handle.clone();
            let projects_dir = self.projects_dir.clone();
            let cache = self.cache.clone();
            let fs_provider = self.fs_provider.clone();
            let file_watcher = self.file_watcher.clone();

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
            let file_watcher_for_error = self.file_watcher.clone();

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

        // === Todo 文件监听器任务（完整实现，R3）===
        {
            let cancel = cancel_token.clone();
            let app = app_handle;
            let todo_fs_provider = self.fs_provider.clone();
            let todos_dir = self.todos_dir.clone();
            let todo_watcher = self.todo_watcher.clone();

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

        cancel_token
    }
}
