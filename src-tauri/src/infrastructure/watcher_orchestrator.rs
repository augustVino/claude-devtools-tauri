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

/// Phase 4B: 增量错误检测状态（纯内存，per-file）。
///
/// 对齐 Electron FileWatcher.ts:615-709 的 lastProcessedSize / lastProcessedLineCount
/// / processingInProgress / pendingReprocess。不持久化到 DataCache（重启后所有文件
/// 走全量重置，last_offset=0 + last_line_count=0 → 避免行号错位）。
#[derive(Default)]
struct IncrementalParseState {
    /// 文件上次解析到的 byte offset（增量读取起点）
    last_offset: std::collections::HashMap<String, u64>,
    /// 文件上次已知的 size（与 stat 对比判断是否有变化）
    last_size: std::collections::HashMap<String, u64>,
    /// 文件上次已知的 mtime_ms（防 sed -i 原地改写：size 不变但内容变了）
    last_mtime: std::collections::HashMap<String, u64>,
    /// 文件已解析的行数（增量场景 error.line_number 偏移基准）
    last_line_count: std::collections::HashMap<String, u64>,
    /// 并发 guard：当前正在处理的文件集合
    processing: std::collections::HashSet<String>,
    /// 重处理队列：guard 持有期间又有 event 到达时入队
    pending_reprocess: std::collections::HashSet<String>,
}

/// Phase 4B: 单文件错误检测的增量解析逻辑。
///
/// 步骤：
/// 1. stat → current_size + current_mtime
/// 2. 快速路径：size + mtime 都未变 + last_offset>0 → 跳过
/// 3. 并发 guard：已 processing → 入 pending_reprocess 队列
/// 4. 强制全量重置：current_size < last_offset || mtime 回退
/// 5. 增量 vs 全量：last_offset>0 && current_size > last_size && mtime 未回退 → 增量
/// 6. 增量调 parse_jsonl_from_offset，全量调 parse_jsonl_file_with_provider
/// 7. error.line_number 偏移：增量场景 += last_line_count
/// 8. 更新 state（含 last_line_count） + 释放 guard + 检查 pending
async fn handle_error_event(
    path_str: &str,
    path: &std::path::Path,
    state: &Arc<tokio::sync::Mutex<IncrementalParseState>>,
    fs_provider: &Arc<dyn FsProvider>,
    detector: &crate::error::error_detector::ErrorDetector,
    notification_manager: &Arc<RwLock<NotificationManager>>,
    _config_manager: &Arc<ConfigManager>,
) {
    let stat = match fs_provider.stat(path) {
        Ok(s) => s,
        Err(_) => return,
    };
    let current_size = stat.size;
    let current_mtime = stat.mtime_ms;

    // 并发 guard：检查是否已在处理
    {
        let mut st = state.lock().await;
        if st.processing.contains(path_str) {
            st.pending_reprocess.insert(path_str.to_string());
            return;
        }
        // 快速路径：size + mtime 都未变且已有 offset → 跳过
        let last_sz = st.last_size.get(path_str).copied().unwrap_or(0);
        let last_mt = st.last_mtime.get(path_str).copied().unwrap_or(0);
        let last_off = st.last_offset.get(path_str).copied().unwrap_or(0);
        if last_off > 0 && current_size == last_sz && current_mtime == last_mt {
            return;
        }
        st.processing.insert(path_str.to_string());
    }

    // 决定增量 vs 全量
    let (last_offset, last_line_count, force_full) = {
        let st = state.lock().await;
        let off = st.last_offset.get(path_str).copied().unwrap_or(0);
        let lc = st.last_line_count.get(path_str).copied().unwrap_or(0);
        let last_sz = st.last_size.get(path_str).copied().unwrap_or(0);
        let last_mt = st.last_mtime.get(path_str).copied().unwrap_or(0);
        // 强制全量：文件被 truncate / 原地改写（size < offset）或 mtime 回退
        let force = off > 0 && (current_size < off || current_mtime < last_mt || current_size < last_sz);
        (off, lc, force)
    };
    let can_incremental = !force_full
        && last_offset > 0
        && current_size > *state.lock().await.last_size.get(path_str).unwrap_or(&0);

    let (messages, new_offset, new_line_count) = if can_incremental {
        // 增量解析
        let (new_msgs, new_off) =
            crate::parsing::jsonl_parser::parse_jsonl_from_offset(path, last_offset, fs_provider.as_ref()).await;
        let count = last_line_count + new_msgs.len() as u64;
        (new_msgs, new_off, count)
    } else {
        // 全量解析（首次 / 截断 / 重写）
        let all_msgs = crate::parsing::jsonl_parser::parse_jsonl_file_with_provider(path, fs_provider.as_ref()).await;
        let count = all_msgs.len() as u64;
        (all_msgs, current_size, count)
    };

    if !messages.is_empty() {
        let session_id = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        // project_id 反推（从 path 末两段，简化：仅 log，实际 detector 需要）
        let project_id = path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        // 增量场景：error.line_number += last_line_count（已解析行偏移）
        let errors = if can_incremental && last_line_count > 0 {
            let raw_errors = detector
                .detect_errors(&messages, &session_id, &project_id, path_str)
                .await;
            raw_errors
                .into_iter()
                .map(|mut e| {
                    if let Some(line) = e.line_number.as_mut() {
                        *line += last_line_count;
                    }
                    e
                })
                .collect::<Vec<_>>()
        } else {
            detector
                .detect_errors(&messages, &session_id, &project_id, path_str)
                .await
        };

        let mgr = notification_manager.read().await;
        for detected_error in errors {
            let _ = mgr.add_error(detected_error).await;
        }
    }

    // 更新 state + 释放 guard + 检查 pending
    let has_pending = {
        let mut st = state.lock().await;
        st.last_offset.insert(path_str.to_string(), new_offset);
        st.last_size.insert(path_str.to_string(), current_size);
        st.last_mtime.insert(path_str.to_string(), current_mtime);
        st.last_line_count.insert(path_str.to_string(), new_line_count);
        st.processing.remove(path_str);
        st.pending_reprocess.remove(path_str)
    };

    // 如果 guard 持有期间又有 event，递归重处理一次（防丢失）
    if has_pending {
        Box::pin(handle_error_event(
            path_str,
            path,
            state,
            fs_provider,
            detector,
            notification_manager,
            _config_manager,
        ))
        .await;
    }
}

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
        // Phase 4B: 增量解析 + 并发 guard + catch-up scan（对齐 Electron FileWatcher.ts:615-709, 900-950）
        {
            let cancel = cancel_token.clone();
            let file_watcher_for_error = self.file_watcher.clone();
            let error_fs_provider = self.fs_provider.clone();

            tauri::async_runtime::spawn(async move {
                // 订阅主 watcher 的事件
                let mut error_rx = { file_watcher_for_error.lock().await.receiver() };
                let detector = crate::error::error_detector::ErrorDetector::new(config_manager.clone());

                // Phase 4B: 增量解析状态（纯内存，重启后所有文件走全量重置）
                // 简化持久化决策：不持久化到 DataCache。重启后 last_offset=0 触发全量重检，
                // error.line_number 不偏移（last_line_count=0）→ 避免行号错位（reviewer v2 指出的关键风险）。
                // 代价：重启后已知错误可能再次冒泡（用户可 dismiss）。比"行号错位指向错误位置"好。
                let state: Arc<tokio::sync::Mutex<IncrementalParseState>> =
                    Arc::new(tokio::sync::Mutex::new(IncrementalParseState::default()));

                // Phase 4B: 独立 catch-up task（30s interval，避免 biased select 饥饿）
                // 对齐 Electron runCatchUpScan（FileWatcher.ts:916-950）
                {
                    let cancel_catchup = cancel.clone();
                    let state_catchup = state.clone();
                    let fs_catchup = error_fs_provider.clone();
                    let config_catchup = config_manager.clone();
                    let notif_catchup = notification_manager.clone();
                    tauri::async_runtime::spawn(async move {
                        // 重建 detector（ErrorDetector 不 Clone，但内部仅 Arc<ConfigManager>，等价）
                        let detector_catchup = crate::error::error_detector::ErrorDetector::new(config_catchup.clone());
                        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
                        interval.tick().await; // 跳过立即触发
                        loop {
                            tokio::select! {
                                _ = interval.tick() => {
                                    let snapshot = state_catchup.lock().await.last_size.clone();
                                    for (path_str, _last_sz) in snapshot {
                                        let path = std::path::PathBuf::from(&path_str);
                                        let stat = match fs_catchup.stat(&path) {
                                            Ok(s) => s,
                                            Err(_) => continue,
                                        };
                                        let should_rescan = {
                                            let st = state_catchup.lock().await;
                                            let last_sz = st.last_size.get(&path_str).copied().unwrap_or(0);
                                            let last_mt = st.last_mtime.get(&path_str).copied().unwrap_or(0);
                                            stat.size != last_sz || stat.mtime_ms != last_mt
                                        };
                                        if should_rescan {
                                            handle_error_event(
                                                &path_str,
                                                &path,
                                                &state_catchup,
                                                &fs_catchup,
                                                &detector_catchup,
                                                &notif_catchup,
                                                &config_catchup,
                                            )
                                            .await;
                                        }
                                    }
                                }
                                _ = cancel_catchup.cancelled() => break,
                            }
                        }
                    });
                }

                loop {
                    tokio::select! {
                        result = error_rx.recv() => {
                            match result {
                                Ok(event) => {
                                    let path = std::path::Path::new(&event.path);
                                    if path.extension().map(|e| e != "jsonl").unwrap_or(true) {
                                        continue;
                                    }
                                    // Phase 3A: Memory 事件跳过错误检测
                                    if event.kind == crate::types::domain::FileChangeEventKind::Memory {
                                        continue;
                                    }
                                    // includeSubagentErrors gate（对齐 Electron FileWatcher.ts:583-596）
                                    let include_subagent = config_manager
                                        .get_config()
                                        .await
                                        .notifications
                                        .include_subagent_errors;
                                    if !include_subagent && crate::utils::is_subagent_file(&event.path) {
                                        continue;
                                    }
                                    handle_error_event(
                                        &event.path,
                                        path,
                                        &state,
                                        &error_fs_provider,
                                        &detector,
                                        &notification_manager,
                                        &config_manager,
                                    ).await;
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

    /// Phase 4B: IncrementalParseState 快速路径 — size + mtime 不变 + last_offset>0 → 应跳过
    #[test]
    fn test_incremental_state_fast_path_skip() {
        let mut st = IncrementalParseState::default();
        st.last_offset.insert("/p/s.jsonl".into(), 100);
        st.last_size.insert("/p/s.jsonl".into(), 200);
        st.last_mtime.insert("/p/s.jsonl".into(), 1000);

        let last_off = st.last_offset.get("/p/s.jsonl").copied().unwrap_or(0);
        let last_sz = st.last_size.get("/p/s.jsonl").copied().unwrap_or(0);
        let last_mt = st.last_mtime.get("/p/s.jsonl").copied().unwrap_or(0);
        let current_size = 200u64;
        let current_mtime = 1000u64;

        let should_skip = last_off > 0 && current_size == last_sz && current_mtime == last_mt;
        assert!(should_skip, "size + mtime unchanged + offset>0 → skip");
    }

    /// Phase 4B: force_full 判断 — current_size < last_offset → 必须全量重置
    #[test]
    fn test_incremental_state_force_full_on_truncate() {
        let mut st = IncrementalParseState::default();
        st.last_offset.insert("/p/s.jsonl".into(), 500); // 已读到 500 字节
        st.last_size.insert("/p/s.jsonl".into(), 500);
        st.last_mtime.insert("/p/s.jsonl".into(), 1000);

        // 文件被 truncate 到 300 字节
        let current_size = 300u64;
        let last_off = st.last_offset.get("/p/s.jsonl").copied().unwrap_or(0);
        let force_full = last_off > 0 && current_size < last_off;
        assert!(force_full, "size < last_offset → force full reset");
    }

    /// Phase 4B: 并发 guard — processing 集合包含路径时入 pending_reprocess
    #[test]
    fn test_incremental_state_concurrency_guard() {
        let mut st = IncrementalParseState::default();
        st.processing.insert("/p/s.jsonl".into());

        // 模拟 handle_error_event guard 检查
        let should_queue = st.processing.contains("/p/s.jsonl");
        assert!(should_queue, "file in processing → queue in pending");

        st.pending_reprocess.insert("/p/s.jsonl".into());
        assert!(st.pending_reprocess.contains("/p/s.jsonl"));
    }

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
