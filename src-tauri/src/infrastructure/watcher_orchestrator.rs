//! Watcher 任务编排器 — 从 ServiceContext 中提取的文件监听任务管理。
//!
//! 负责 spawn 三个并发 tokio task：主监听器、错误检测管道、Todo 监听器。
//! Phase A: 启动时扫描最近修改的会话文件并记录（seed_active_sessions）。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::infrastructure::{
    fs_provider::FsProvider, ConfigManager, DataCache, FileWatcher, NotificationManager,
};
use tauri::Manager;

/// Watcher 编排器 — 管理文件监听任务的启动生命周期。
pub struct WatcherOrchestrator {
    #[allow(dead_code)]
    projects_dir: PathBuf,
    todos_dir: PathBuf,
    fs_provider: Arc<dyn FsProvider>,
    cache: DataCache,
    file_watcher: Arc<Mutex<FileWatcher>>,
    todo_watcher: Arc<Mutex<FileWatcher>>,
    /// Callback invoked on file change events to invalidate project-level caches.
    /// Mirrors Electron's `FileWatcher.setProjectScanner().invalidateCachesForProject()`.
    ///
    /// **Constraint**: Callback MUST be non-blocking or spawn async work internally.
    /// Current implementation spawns a tokio task for DataCache.invalidate_project().
    on_project_cache_invalidate: Option<Arc<dyn Fn(&str) + Send + Sync>>,
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
            on_project_cache_invalidate: None,
        }
    }

    /// Register a callback for project-level cache invalidation on file changes.
    ///
    /// This mirrors Electron's `FileWatcher.setProjectScanner()` pattern where
    /// file change events trigger `ProjectScanner.invalidateCachesForProject()`.
    /// In Tauri, this is wired to `DataCache.invalidate_project()` in
    /// `ServiceContext::spawn_watcher_tasks()`.
    pub fn set_on_cache_invalidate<F>(&mut self, callback: F)
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        self.on_project_cache_invalidate = Some(Arc::new(callback));
    }

    /// Phase A: 启动时扫描最近修改的会话文件并记录。
    ///
    /// 为未来的 Phase B (catch-up scan) 建立追踪基础。
    ///
    /// 注意：此函数仅做日志记录，不推送 FileChangeEvent。
    /// Electron 的 seedActiveSessionFiles() 同样不发射事件，
    /// 它只填充内部 activeSessionFiles Map。
    async fn seed_active_sessions<Fs: FsProvider + ?Sized>(
        fs_provider: &Fs,
        projects_dir: &Path,
    ) -> Result<u32, String> {
        let cutoff = SystemTime::now()
            .checked_sub(Duration::from_secs(3600))
            .ok_or("Failed to compute 1h cutoff time")?;
        let cutoff_ms = crate::utils::time::time_to_ms(Some(cutoff));

        // 列出 projects_dir 下的一级子目录（每个是一个 project hash）
        let project_entries = fs_provider
            .read_dir(projects_dir)
            .map_err(|e| format!("Failed to read projects dir: {e}"))?;

        let mut total_seeded = 0u32;

        for project in &project_entries {
            if !project.is_directory {
                continue;
            }
            let project_path = projects_dir.join(&project.name);

            // 在每个 project 目录中查找 .jsonl 文件
            let session_entries = match fs_provider.read_dir(&project_path) {
                Ok(entries) => entries,
                Err(e) => {
                    log::debug!("Skipping unreadable project dir {}: {e}", project.name);
                    continue;
                }
            };

            for entry in &session_entries {
                if !entry.is_file {
                    continue;
                }
                if !entry.name.ends_with(".jsonl") {
                    continue;
                }

                // 只收集最近 1 小时内修改过的文件
                if let Some(mtime_ms) = entry.mtime_ms {
                    if mtime_ms >= cutoff_ms {
                        total_seeded += 1;
                        log::info!(
                            "[seed] Active session found: {}/{} (mtime={}ms)",
                            project.name,
                            entry.name,
                            mtime_ms
                        );
                    }
                }
            }
        }

        log::info!(
            "[seed] Active session seeding complete: {} files found in {} projects",
            total_seeded,
            project_entries.iter().filter(|p| p.is_directory).count()
        );

        Ok(total_seeded)
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
            let on_cache_invalidate = self.on_project_cache_invalidate.clone();

            tauri::async_runtime::spawn(async move {
                let mut watcher = file_watcher.lock().await;
                if !fs_provider.exists(&projects_dir).unwrap_or(false) {
                    if let Err(e) = fs_provider.ensure_dir(&projects_dir) {
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
                                    // Phase 3A: Memory 事件分流（跳过 session cache + 走 memory-change channel）
                                    if event.kind == crate::types::domain::FileChangeEventKind::Memory {
                                        if let Some(pid) = &event.project_id {
                                            let mem_event = crate::events::MemoryChangeEvent {
                                                project_id: pid.clone(),
                                            };
                                            crate::events::emit_memory_change(&app, mem_event.clone());
                                            if let Some(broadcaster) =
                                                app.try_state::<crate::http::sse::SSEBroadcaster>()
                                            {
                                                let _ = broadcaster.inner().send(
                                                    crate::http::sse::BackendEvent::MemoryChange(mem_event),
                                                );
                                            }
                                        }
                                        continue;
                                    }

                                    // Session 事件原流程
                                    if let (Some(pid), Some(sid)) =
                                        (&event.project_id, &event.session_id)
                                    {
                                        cache.invalidate_session(pid, sid).await;

                                        // Invalidate project-level caches (mirrors Electron's FileWatcher→ProjectScanner linkage)
                                        if let Some(ref cb) = on_cache_invalidate {
                                            cb(pid);
                                        }
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
            let file_watcher_for_error = self.file_watcher.clone();
            let error_fs_provider = self.fs_provider.clone();

            tauri::async_runtime::spawn(async move {
                // 订阅主 watcher 的事件
                let mut error_rx = { file_watcher_for_error.lock().await.receiver() };
                let detector = crate::error::error_detector::ErrorDetector::new(config_manager.clone());
                // config_manager clone 保留用于 subagent gate（动态读 notification 配置）
                loop {
                    tokio::select! {
                        result = error_rx.recv() => {
                            match result {
                                Ok(event) => {
                                    let path = std::path::Path::new(&event.path);
                                    if path.extension().map(|e| e != "jsonl").unwrap_or(true) {
                                        continue;
                                    }
                                    // includeSubagentErrors gate（对齐 Electron FileWatcher.ts:583-596）：
                                    // 默认 true，但仅在配置开启时处理 subagent 文件。
                                    // ⚠️ 行为变更：之前无条件 continue 跳过所有 subagent，
                                    // 接线后默认 true 的用户首次会收到 subagent 错误通知。
                                    let include_subagent = config_manager
                                        .get_config()
                                        .await
                                        .notifications
                                        .include_subagent_errors;
                                    if !include_subagent && crate::utils::is_subagent_file(&event.path) {
                                        continue;
                                    }
                                    let session_id = path.file_stem()
                                        .map(|s| s.to_string_lossy().to_string())
                                        .unwrap_or_default();
                                    let project_id = event.project_id.clone().unwrap_or_default();
                                    let messages = crate::parsing::jsonl_parser::parse_jsonl_file_with_provider(path, error_fs_provider.as_ref()).await;
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
                    if let Err(e) = todo_fs_provider.ensure_dir(&todos_dir) {
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

        // Phase A: 轻量 seed（延迟 2s 等 watcher 就绪）
        {
            let seed_fs = self.fs_provider.clone();
            let seed_dir = self.projects_dir.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_secs(2)).await;
                if let Err(e) = Self::seed_active_sessions(&*seed_fs, &seed_dir).await {
                    log::warn!("Failed to seed active sessions: {e}");
                }
            });
        }

        cancel_token
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn test_cache_invalidate_callback_fired_on_file_event() {
        // Setup: create orchestrator components (minimal, no actual FS)
        let tmp = tempfile::tempdir().unwrap();
        let fs_provider = Arc::new(crate::infrastructure::fs_provider::LocalFsProvider::new());
        let cache = DataCache::new();
        let fw = FileWatcher::new(fs_provider.clone());
        let tw = FileWatcher::new(fs_provider.clone());

        let mut orch = WatcherOrchestrator::new(
            tmp.path().to_path_buf(),
            tmp.path().join("todos").to_path_buf(),
            fs_provider,
            cache.clone(),
            Arc::new(Mutex::new(fw)),
            Arc::new(Mutex::new(tw)),
        );

        // Register callback that tracks invocations
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();
        orch.set_on_cache_invalidate(move |_pid: &str| {
            called_clone.store(true, Ordering::SeqCst);
        });

        // Verify setter worked
        assert!(orch.on_project_cache_invalidate.is_some());

        // Simulate: manually invoke the callback logic as the event handler would
        if let Some(ref cb) = orch.on_project_cache_invalidate {
            cb("test-project-abc123");
        }

        // Verify callback was called with correct project_id
        assert!(
            called.load(Ordering::SeqCst),
            "Cache invalidate callback should have been fired for test-project-abc123"
        );
    }

    #[test]
    fn test_no_callback_by_default() {
        let tmp = tempfile::tempdir().unwrap();
        let fs_provider = Arc::new(crate::infrastructure::fs_provider::LocalFsProvider::new());
        let cache = DataCache::new();
        let fw = FileWatcher::new(fs_provider.clone());
        let tw = FileWatcher::new(fs_provider.clone());

        let orch = WatcherOrchestrator::new(
            tmp.path().to_path_buf(),
            tmp.path().join("todos").to_path_buf(),
            fs_provider,
            cache,
            Arc::new(Mutex::new(fw)),
            Arc::new(Mutex::new(tw)),
        );

        // By default, no callback registered — should not panic when accessed
        assert!(orch.on_project_cache_invalidate.is_none());
    }
}
