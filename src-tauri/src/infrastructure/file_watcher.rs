//! 文件监听器 — 监听 Claude Code 项目文件的变化。
//!
//! 职责:
//! - 监听目录中会话文件的变更
//! - 对快速连续的文件事件进行防抖（100ms）
//! - 解析路径以提取 projectId、sessionId、isSubagent
//! - 向订阅者广播 FileChangeEvent

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

// 从 notify_debouncer_mini 导入所有 notify 类型，以确保版本兼容性
// (notify-debouncer-mini 使用 notify v7，而直接依赖为 v8)
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, DebouncedEvent};

use crate::infrastructure::fs_provider::FsProvider;
use crate::types::domain::{FileChangeEvent, FileChangeType};

/// 防抖间隔（毫秒），与 Electron 实现保持一致
const DEBOUNCE_MS: u64 = 100;

/// 文件变更事件的广播通道容量
const CHANNEL_CAPACITY: usize = 64;

/// SSH 轮询间隔（毫秒），与 Electron SSH_POLL_INTERVAL_MS 保持一致
const SSH_POLL_INTERVAL_MS: u64 = 3000;

/// 测试用轮询间隔（毫秒），避免测试等待 3 秒
#[cfg(test)]
const TEST_POLL_INTERVAL_MS: u64 = 50;

/// 文件监听模式。
#[derive(Debug, Clone, PartialEq)]
enum WatchMode {
    /// 本地模式：使用 notify_debouncer_mini (OS 级文件事件)
    Local,
    /// SSH 轮询模式：通过 FsProvider 定期扫描文件变更
    SshPolling,
}

/// SSH 轮询状态。
struct SshPollState {
    /// 轮询任务句柄（Option: 无轮询时为 None）
    timer: Option<JoinHandle<()>>,
    /// 轮询间隔（毫秒）
    poll_interval_ms: u64,
    /// 首次基线扫描是否已完成
    primed: bool,
    /// 已追踪的文件大小 {绝对路径 -> 字节数}
    polled_file_sizes: HashMap<String, u64>,
    /// 防止重叠轮询的 guard
    poll_in_progress: bool,
}

/// 文件监听器，以防抖方式监听目录中的文件变更。
pub struct FileWatcher {
    /// 监听模式（本地 or SSH 轮询）
    mode: WatchMode,
    /// 文件系统提供者
    fs_provider: Arc<dyn FsProvider>,
    /// 文件变更事件的广播发送端
    sender: broadcast::Sender<FileChangeEvent>,
    /// 是否正在监听
    is_watching: Arc<Mutex<bool>>,
    /// 本地模式：防抖监听器
    local_watcher: Arc<Mutex<Option<DebouncedWatcher>>>,
    /// SSH 模式：轮询状态
    ssh_poll_state: Arc<Mutex<SshPollState>>,
}

/// 防抖监听器句柄的包装结构
struct DebouncedWatcher {
    #[allow(dead_code)]
    watcher: notify_debouncer_mini::Debouncer<RecommendedWatcher>,
}

impl FileWatcher {
    /// 创建新的 FileWatcher 实例。
    pub fn new(fs_provider: Arc<dyn FsProvider>) -> Self {
        let mode = match fs_provider.provider_type() {
            "ssh" => WatchMode::SshPolling,
            _ => WatchMode::Local,
        };
        let poll_interval_ms = SSH_POLL_INTERVAL_MS;
        let (sender, _) = broadcast::channel(CHANNEL_CAPACITY);
        Self {
            mode,
            fs_provider,
            sender,
            is_watching: Arc::new(Mutex::new(false)),
            local_watcher: Arc::new(Mutex::new(None)),
            ssh_poll_state: Arc::new(Mutex::new(SshPollState {
                timer: None,
                poll_interval_ms,
                primed: false,
                polled_file_sizes: HashMap::new(),
                poll_in_progress: false,
            })),
        }
    }

    /// 仅用于测试：创建带自定义轮询间隔的 FileWatcher。
    #[cfg(test)]
    fn with_poll_interval(fs_provider: Arc<dyn FsProvider>, interval_ms: u64) -> Self {
        let mode = match fs_provider.provider_type() {
            "ssh" => WatchMode::SshPolling,
            _ => WatchMode::Local,
        };
        let (sender, _) = broadcast::channel(CHANNEL_CAPACITY);
        Self {
            mode,
            fs_provider,
            sender,
            is_watching: Arc::new(Mutex::new(false)),
            local_watcher: Arc::new(Mutex::new(None)),
            ssh_poll_state: Arc::new(Mutex::new(SshPollState {
                timer: None,
                poll_interval_ms: interval_ms,
                primed: false,
                polled_file_sizes: HashMap::new(),
                poll_in_progress: false,
            })),
        }
    }

    /// 启动对指定目录的监听。
    ///
    /// 根据当前模式选择本地 OS 级监听或 SSH 轮询。
    /// 仅对 `.jsonl` 和 `.json` 文件发出事件。
    /// 解析路径以提取 projectId、sessionId 和 isSubagent。
    ///
    /// 路径模式:
    /// - 会话文件: `watchPath/projectId/sessionId.jsonl`
    /// - 子代理文件: `watchPath/projectId/sessionId/subagents/agent-hash.jsonl`
    pub async fn watch(&mut self, path: &Path) -> Result<(), String> {
        match self.mode {
            WatchMode::Local => self.watch_local(path).await,
            WatchMode::SshPolling => self.start_ssh_polling(path).await,
        }
    }

    /// 本地模式：使用 notify_debouncer_mini 监听文件变更。
    async fn watch_local(&mut self, path: &Path) -> Result<(), String> {
        if !self
            .fs_provider
            .exists(path)
            .map_err(|e| format!("Path check failed: {}", e))?
        {
            return Err(format!("Path does not exist: {}", path.display()));
        }

        let mut is_watching = self.is_watching.lock().await;
        if *is_watching {
            return Err("Already watching a directory".to_string());
        }

        let sender = self.sender.clone();
        let watch_path = path.to_path_buf();
        let fs_provider = self.fs_provider.clone();

        // 创建防抖监听器及事件通道
        let (tx, mut rx) = tokio::sync::mpsc::channel::<DebouncedEvent>(64);

        let mut debouncer = new_debouncer(Duration::from_millis(DEBOUNCE_MS), move |result| {
            if let Ok(events) = result {
                for event in events {
                    let _ = tx.blocking_send(event);
                }
            }
        })
        .map_err(|e| format!("Failed to create debouncer: {}", e))?;

        debouncer
            .watcher()
            .watch(&watch_path, RecursiveMode::Recursive)
            .map_err(|e| format!("Failed to start watcher: {}", e))?;

        // 启动异步任务处理防抖后的事件
        tokio::spawn(async move {
            while let Some(debounced_event) = rx.recv().await {
                if let Some(change_event) =
                    Self::process_debounced_event_with_provider(&fs_provider, &debounced_event, &watch_path)
                {
                    let _ = sender.send(change_event);
                }
            }
        });

        *self.local_watcher.lock().await = Some(DebouncedWatcher { watcher: debouncer });
        *is_watching = true;

        log::info!("FileWatcher: Started local watching {}", path.display());
        Ok(())
    }

    /// 停止监听目录，清理所有模式的状态。
    pub async fn stop(&mut self) {
        // 本地模式：丢弃防抖监听器
        if self.local_watcher.lock().await.take().is_some() {
            log::info!("FileWatcher: Stopped local watcher");
        }

        // SSH 轮询模式：取消轮询任务并重置状态
        {
            let mut state = self.ssh_poll_state.lock().await;
            if let Some(handle) = state.timer.take() {
                handle.abort();
                log::info!("FileWatcher: Stopped SSH polling");
            }
            state.primed = false;
            state.polled_file_sizes.clear();
            state.poll_in_progress = false;
        }

        *self.is_watching.lock().await = false;
    }

    /// 停止当前监听并切换到新目录。
    ///
    /// 用于上下文切换时重新配置监听路径。
    pub async fn rewatch(&mut self, new_path: &Path) -> Result<(), String> {
        self.stop().await;
        self.watch(new_path).await
    }

    /// 返回文件变更事件的广播接收端。
    pub fn receiver(&self) -> broadcast::Receiver<FileChangeEvent> {
        self.sender.subscribe()
    }

    /// 检查当前是否正在监听目录。
    pub async fn is_watching(&self) -> bool {
        *self.is_watching.lock().await
    }

    /// 处理防抖事件并转换为 FileChangeEvent（使用 FsProvider 检查文件存在性）。
    ///
    /// 解析相对于监听根目录的文件路径，提取:
    /// - projectId: 监听路径后的第一级目录
    /// - sessionId: 路径中的会话 ID
    /// - isSubagent: 是否为子代理文件
    ///
    /// 路径模式（与 Electron FileWatcher.ts 一致）:
    /// - 会话文件: `watchPath/projectId/sessionId.jsonl`
    /// - 子代理文件: `watchPath/projectId/sessionId/subagents/agent-hash.jsonl`
    fn process_debounced_event_with_provider(
        fs_provider: &Arc<dyn FsProvider>,
        event: &DebouncedEvent,
        watch_path: &Path,
    ) -> Option<FileChangeEvent> {
        // 仅处理 .jsonl 和 .json 文件
        let extension = event.path.extension()?.to_str()?;

        if extension != "jsonl" && extension != "json" {
            return None;
        }

        // 通过 FsProvider 检查文件是否存在来判断是新增/修改还是删除
        // (debouncer-mini 不区分事件类型 — 仅返回 Any/AnyContinuous)
        let event_type = if fs_provider.exists(&event.path).unwrap_or(false) {
            // 文件存在: 可能是新增或修改
            // 统一报告 "change"（Electron 也对大多数情况报告 "change"）
            FileChangeType::Change
        } else {
            // 文件不存在: 已被删除
            FileChangeType::Unlink
        };

        // 解析相对路径以提取 projectId、sessionId、isSubagent
        let relative_path = event.path.strip_prefix(watch_path).ok()?;
        let parts: Vec<&str> = relative_path
            .components()
            .filter_map(|c| c.as_os_str().to_str())
            .collect();

        let (project_id, session_id, is_subagent) = Self::parse_path_parts(&parts);

        Some(FileChangeEvent {
            event_type,
            path: event.path.to_string_lossy().to_string(),
            project_id,
            session_id,
            is_subagent,
        })
    }

    /// 解析路径分段，提取 projectId、sessionId 和 isSubagent。
    ///
    /// 与 Electron FileWatcher.ts 逻辑一致（第 507-533 行）:
    /// - 会话文件（2 段）: `projectId/sessionId.jsonl`
    /// - 子代理文件（4 段）: `projectId/sessionId/subagents/agent-hash.jsonl`
    fn parse_path_parts(parts: &[&str]) -> (Option<String>, Option<String>, bool) {
        if parts.is_empty() {
            return (None, None, false);
        }

        let project_id = Some(parts[0].to_string());

        // 项目根目录下的会话文件: projectId/sessionId.jsonl
        if parts.len() == 2 && parts[1].ends_with(".jsonl") {
            let session_id = parts[1].strip_suffix(".jsonl").map(|s| s.to_string());
            return (project_id, session_id, false);
        }

        // 子代理文件: projectId/sessionId/subagents/agent-hash.jsonl
        if parts.len() == 4 && parts[2] == "subagents" && parts[3].ends_with(".jsonl") {
            let session_id = parts[1].to_string();
            return (project_id, Some(session_id), true);
        }

        (project_id, None, false)
    }

    /// 启动 SSH 轮询模式。
    async fn start_ssh_polling(&mut self, path: &Path) -> Result<(), String> {
        if !self
            .fs_provider
            .exists(path)
            .map_err(|e| format!("SSH exists check: {}", e))?
        {
            return Err(format!("Path does not exist (SSH): {}", path.display()));
        }

        let mut is_watching = self.is_watching.lock().await;
        if *is_watching {
            return Err("Already watching a directory".to_string());
        }

        let sender = self.sender.clone();
        let fs_provider = self.fs_provider.clone();
        let poll_state = self.ssh_poll_state.clone();
        let projects_path = path.to_path_buf();

        // 读取间隔（允许测试覆盖）
        let poll_interval = self.ssh_poll_state.lock().await.poll_interval_ms;

        // 重置状态以确保全新开始
        {
            let mut state = poll_state.lock().await;
            state.primed = false;
            state.polled_file_sizes.clear();
            state.poll_in_progress = false;
        }

        let handle = tokio::spawn(async move {
            // 立即执行首次基线扫描
            Self::poll_for_changes(&fs_provider, &projects_path, &poll_state, &sender).await;

            loop {
                tokio::time::sleep(Duration::from_millis(poll_interval)).await;
                Self::poll_for_changes(&fs_provider, &projects_path, &poll_state, &sender).await;
            }
        });

        self.ssh_poll_state.lock().await.timer = Some(handle);
        *is_watching = true;

        log::info!(
            "FileWatcher: Started SSH polling {} (interval={}ms)",
            path.display(),
            poll_interval
        );
        Ok(())
    }

    /// 执行一次 SSH 轮询扫描。
    async fn poll_for_changes(
        fs_provider: &Arc<dyn FsProvider>,
        projects_path: &Path,
        poll_state: &Arc<Mutex<SshPollState>>,
        sender: &broadcast::Sender<FileChangeEvent>,
    ) {
        // Guard: 防止重叠轮询
        {
            let mut state = poll_state.lock().await;
            if state.poll_in_progress {
                return;
            }
            state.poll_in_progress = true;
        }

        let result = Self::do_poll(fs_provider, projects_path, poll_state, sender).await;

        poll_state.lock().await.poll_in_progress = false;

        if let Err(e) = result {
            log::error!("SSH poll error: {}", e);
        }
    }

    /// 实际的轮询逻辑。
    ///
    /// 注意：SSH 轮询只做 2 层 readdir（projects/ → projectId/），
    /// 与 Electron 行为一致。不递归进入 subagents/ 子目录。
    async fn do_poll(
        fs_provider: &Arc<dyn FsProvider>,
        projects_path: &Path,
        poll_state: &Arc<Mutex<SshPollState>>,
        sender: &broadcast::Sender<FileChangeEvent>,
    ) -> Result<(), String> {
        let mut state = poll_state.lock().await;
        let primed = state.primed;
        let mut seen_files = HashSet::new();
        let mut pending_events: Vec<(std::path::PathBuf, FileChangeType)> = Vec::new();

        // 读取项目目录列表
        let project_dirs = fs_provider
            .read_dir(projects_path)
            .map_err(|e| format!("SSH read_dir failed: {}", e))?;

        for dir in &project_dirs {
            if !dir.is_directory {
                continue;
            }
            let project_path = projects_path.join(&dir.name);

            let entries = match fs_provider.read_dir(&project_path) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for entry in &entries {
                if !entry.is_file || !entry.name.ends_with(".jsonl") {
                    continue;
                }
                let full_path = project_path.join(&entry.name);
                let path_str = full_path.to_string_lossy().to_string();
                seen_files.insert(path_str.clone());

                let observed_size = entry
                    .size
                    .or_else(|| fs_provider.stat(&full_path).ok().map(|s| s.size))
                    .unwrap_or(0);

                match state.polled_file_sizes.get(&path_str) {
                    None => {
                        state.polled_file_sizes.insert(path_str.clone(), observed_size);
                        if primed {
                            pending_events.push((full_path, FileChangeType::Add));
                        }
                    }
                    Some(&last_size) if observed_size != last_size => {
                        state.polled_file_sizes.insert(path_str, observed_size);
                        pending_events.push((full_path, FileChangeType::Change));
                    }
                    _ => {} // 无变化
                }
            }
        }

        // 删除检测（仅基线之后）
        if primed {
            let removed: Vec<String> = state
                .polled_file_sizes
                .keys()
                .filter(|k| !seen_files.contains(*k))
                .cloned()
                .collect();
            for removed_path in removed {
                state.polled_file_sizes.remove(&removed_path);
                pending_events.push((
                    std::path::PathBuf::from(&removed_path),
                    FileChangeType::Unlink,
                ));
            }
        } else {
            state.primed = true;
        }

        // 统一释放锁后发送事件（减少锁竞争）
        drop(state);
        for (path, event_type) in pending_events {
            Self::emit_event(sender, &path, projects_path, event_type);
        }

        Ok(())
    }

    /// 构造并发送 FileChangeEvent。
    fn emit_event(
        sender: &broadcast::Sender<FileChangeEvent>,
        file_path: &Path,
        projects_path: &Path,
        event_type: FileChangeType,
    ) {
        let relative = match file_path.strip_prefix(projects_path) {
            Ok(r) => r,
            Err(_) => return,
        };
        let parts: Vec<&str> = relative
            .components()
            .filter_map(|c| c.as_os_str().to_str())
            .collect();
        let (project_id, session_id, is_subagent) = Self::parse_path_parts(&parts);

        let event = FileChangeEvent {
            event_type,
            path: file_path.to_string_lossy().to_string(),
            project_id,
            session_id,
            is_subagent,
        };
        let _ = sender.send(event);
    }
}

impl Default for FileWatcher {
    fn default() -> Self {
        Self::new(Arc::new(crate::infrastructure::fs_provider::LocalFsProvider::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::fs_provider::{FsDirent, FsStatResult, LocalFsProvider};
    use std::sync::Mutex as StdMutex;

    fn local_provider() -> Arc<dyn FsProvider> {
        Arc::new(LocalFsProvider::new())
    }

    // ── MockFsProvider ──────────────────────────────────────────

    #[derive(Debug)]
    struct MockFsProvider {
        provider_type_str: &'static str,
        entries: Arc<StdMutex<HashMap<String, Vec<MockDirent>>>>,
    }

    #[derive(Debug, Clone)]
    struct MockDirent {
        name: String,
        is_file: bool,
        is_directory: bool,
        size: Option<u64>,
    }

    impl MockFsProvider {
        fn new(provider_type_str: &'static str) -> Self {
            Self {
                provider_type_str,
                entries: Arc::new(StdMutex::new(HashMap::new())),
            }
        }

        fn set_entries(&self, path: &str, dirents: Vec<MockDirent>) {
            self.entries
                .lock()
                .unwrap()
                .insert(path.to_string(), dirents);
        }

        fn clear_entries(&self) {
            self.entries.lock().unwrap().clear();
        }
    }

    impl FsProvider for MockFsProvider {
        fn provider_type(&self) -> &'static str {
            self.provider_type_str
        }
        fn exists(&self, _path: &std::path::Path) -> Result<bool, String> {
            Ok(true)
        }
        fn read_file(&self, _path: &std::path::Path) -> Result<String, String> {
            Ok(String::new())
        }
        fn read_file_head(
            &self,
            _path: &std::path::Path,
            _max_lines: usize,
        ) -> Result<String, String> {
            Ok(String::new())
        }
        fn stat(&self, _path: &std::path::Path) -> Result<FsStatResult, String> {
            Ok(FsStatResult {
                size: 100,
                mtime_ms: 0,
                birthtime_ms: 0,
                is_file: true,
                is_directory: false,
            })
        }
        fn read_dir(&self, path: &std::path::Path) -> Result<Vec<FsDirent>, String> {
            let key = path.to_string_lossy().to_string();
            let entries = self.entries.lock().unwrap();
            entries
                .get(&key)
                .map(|ents| {
                    ents.iter()
                        .map(|e| FsDirent {
                            name: e.name.clone(),
                            is_file: e.is_file,
                            is_directory: e.is_directory,
                            size: e.size,
                            mtime_ms: None,
                            birthtime_ms: None,
                        })
                        .collect()
                })
                .ok_or_else(|| format!("No mock entries for {}", key))
        }
    }

    #[tokio::test]
    async fn test_create_watcher() {
        let watcher = FileWatcher::new(local_provider());
        assert!(!watcher.is_watching().await);
    }

    #[tokio::test]
    async fn test_watch_nonexistent_path() {
        let mut watcher = FileWatcher::new(local_provider());
        let result = watcher.watch(Path::new("/nonexistent/path/12345")).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[tokio::test]
    async fn test_stop_without_watch() {
        let mut watcher = FileWatcher::new(local_provider());
        // 不应 panic
        watcher.stop().await;
        assert!(!watcher.is_watching().await);
    }

    #[test]
    fn test_receiver_creation() {
        let watcher = FileWatcher::new(local_provider());
        let _receiver = watcher.receiver();
    }

    #[test]
    fn test_parse_path_parts_session_file() {
        // 会话文件: projectId/sessionId.jsonl
        let parts = vec!["-Users-name-project", "session-abc123.jsonl"];
        let (project_id, session_id, is_subagent) = FileWatcher::parse_path_parts(&parts);

        assert_eq!(project_id, Some("-Users-name-project".to_string()));
        assert_eq!(session_id, Some("session-abc123".to_string()));
        assert!(!is_subagent);
    }

    #[test]
    fn test_parse_path_parts_subagent_file() {
        // 子代理文件: projectId/sessionId/subagents/agent-hash.jsonl
        let parts = vec![
            "-Users-name-project",
            "session-abc123",
            "subagents",
            "agent-def456.jsonl",
        ];
        let (project_id, session_id, is_subagent) = FileWatcher::parse_path_parts(&parts);

        assert_eq!(project_id, Some("-Users-name-project".to_string()));
        assert_eq!(session_id, Some("session-abc123".to_string()));
        assert!(is_subagent);
    }

    #[test]
    fn test_parse_path_parts_empty() {
        let parts: Vec<&str> = vec![];
        let (project_id, session_id, is_subagent) = FileWatcher::parse_path_parts(&parts);

        assert_eq!(project_id, None);
        assert_eq!(session_id, None);
        assert!(!is_subagent);
    }

    #[test]
    fn test_parse_path_parts_only_project() {
        // 仅项目目录，无会话文件
        let parts = vec!["-Users-name-project"];
        let (project_id, session_id, is_subagent) = FileWatcher::parse_path_parts(&parts);

        assert_eq!(project_id, Some("-Users-name-project".to_string()));
        assert_eq!(session_id, None);
        assert!(!is_subagent);
    }

    #[test]
    fn test_parse_path_parts_nested_directory() {
        // 非子代理文件的嵌套目录
        let parts = vec!["-Users-name-project", "some-dir", "other.jsonl"];
        let (project_id, session_id, is_subagent) = FileWatcher::parse_path_parts(&parts);

        assert_eq!(project_id, Some("-Users-name-project".to_string()));
        assert_eq!(session_id, None);
        assert!(!is_subagent);
    }

    // ── SSH 轮询模式测试 ─────────────────────────────────────────

    #[tokio::test]
    async fn test_ssh_watcher_mode_selection() {
        let ssh_provider = Arc::new(MockFsProvider::new("ssh"));
        let watcher = FileWatcher::new(ssh_provider);
        assert_eq!(watcher.mode, WatchMode::SshPolling);

        let local_provider = Arc::new(MockFsProvider::new("local"));
        let local_watcher = FileWatcher::new(local_provider);
        assert_eq!(local_watcher.mode, WatchMode::Local);
    }

    #[tokio::test]
    async fn test_ssh_poll_baseline_priming() {
        let provider = Arc::new(MockFsProvider::new("ssh"));
        provider.set_entries(
            "/projects",
            vec![MockDirent {
                name: "proj1".into(),
                is_file: false,
                is_directory: true,
                size: None,
            }],
        );
        provider.set_entries(
            "/projects/proj1",
            vec![MockDirent {
                name: "session-abc.jsonl".into(),
                is_file: true,
                is_directory: false,
                size: Some(1000),
            }],
        );
        let mut watcher =
            FileWatcher::with_poll_interval(provider.clone(), TEST_POLL_INTERVAL_MS);
        let mut rx = watcher.receiver();

        watcher
            .start_ssh_polling(std::path::Path::new("/projects"))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // 基线扫描不应产生事件
        assert!(rx.try_recv().is_err());
        watcher.stop().await;
    }

    #[tokio::test]
    async fn test_ssh_poll_detects_new_file() {
        let provider = Arc::new(MockFsProvider::new("ssh"));
        provider.set_entries(
            "/projects",
            vec![MockDirent {
                name: "proj1".into(),
                is_file: false,
                is_directory: true,
                size: None,
            }],
        );
        provider.set_entries(
            "/projects/proj1",
            vec![MockDirent {
                name: "session-existing.jsonl".into(),
                is_file: true,
                is_directory: false,
                size: Some(500),
            }],
        );
        let mut watcher =
            FileWatcher::with_poll_interval(provider.clone(), TEST_POLL_INTERVAL_MS);
        let mut rx = watcher.receiver();

        watcher
            .start_ssh_polling(std::path::Path::new("/projects"))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        while rx.try_recv().is_ok() {} // 排空基线（无事件）

        // 添加新文件 — 下一次轮询（50ms）将检测到
        provider.set_entries(
            "/projects/proj1",
            vec![
                MockDirent {
                    name: "session-existing.jsonl".into(),
                    is_file: true,
                    is_directory: false,
                    size: Some(500),
                },
                MockDirent {
                    name: "session-new.jsonl".into(),
                    is_file: true,
                    is_directory: false,
                    size: Some(200),
                },
            ],
        );

        let event = rx
            .recv()
            .await
            .expect("Should receive Add event within 100ms");
        assert_eq!(event.event_type, FileChangeType::Add);
        assert_eq!(event.session_id.as_deref(), Some("session-new"));

        watcher.stop().await;
    }

    #[tokio::test]
    async fn test_rewatch_local_mode() {
        let local_provider = Arc::new(MockFsProvider::new("local"));
        let mut watcher = FileWatcher::new(local_provider);
        assert_eq!(watcher.mode, WatchMode::Local);

        // Watch a temp dir (local mode) — will use notify
        let tmp = std::env::temp_dir().join("file_watcher_test_rewatch");
        let _ = std::fs::create_dir_all(&tmp);
        watcher.watch(&tmp).await.unwrap();
        assert!(watcher.is_watching().await);

        // Rewatch should work — stop + watch on same path
        watcher.rewatch(&tmp).await.unwrap();
        assert!(watcher.is_watching().await);
        watcher.stop().await;
        assert!(!watcher.is_watching().await);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ── Task 5: Comprehensive SSH polling tests ────────────────────

    /// Step 1: File change detection (size increase).
    ///
    /// After baseline, update a file's size in mock entries.
    /// Wait for next poll. Verify `FileChangeType::Change` event with correct session_id.
    #[tokio::test]
    async fn test_ssh_poll_detects_file_change() {
        let provider = Arc::new(MockFsProvider::new("ssh"));
        provider.set_entries(
            "/projects",
            vec![MockDirent {
                name: "proj1".into(),
                is_file: false,
                is_directory: true,
                size: None,
            }],
        );
        provider.set_entries(
            "/projects/proj1",
            vec![MockDirent {
                name: "session-abc.jsonl".into(),
                is_file: true,
                is_directory: false,
                size: Some(1000),
            }],
        );
        let mut watcher =
            FileWatcher::with_poll_interval(provider.clone(), TEST_POLL_INTERVAL_MS);
        let mut rx = watcher.receiver();

        watcher
            .start_ssh_polling(std::path::Path::new("/projects"))
            .await
            .unwrap();
        // Wait for baseline poll to complete
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        while rx.try_recv().is_ok() {} // drain baseline (no events)

        // Simulate file size change (append to session file)
        provider.set_entries(
            "/projects/proj1",
            vec![MockDirent {
                name: "session-abc.jsonl".into(),
                is_file: true,
                is_directory: false,
                size: Some(2500), // was 1000
            }],
        );

        let event = rx
            .recv()
            .await
            .expect("Should receive Change event after size increase");
        assert_eq!(event.event_type, FileChangeType::Change);
        assert_eq!(event.session_id.as_deref(), Some("session-abc"));
        assert_eq!(
            event.project_id.as_deref(),
            Some("proj1")
        );

        watcher.stop().await;
    }

    /// Step 2: File deletion detection.
    ///
    /// After baseline, remove a file from mock entries.
    /// Wait for next poll. Verify `FileChangeType::Unlink` event.
    #[tokio::test]
    async fn test_ssh_poll_detects_file_deletion() {
        let provider = Arc::new(MockFsProvider::new("ssh"));
        provider.set_entries(
            "/projects",
            vec![MockDirent {
                name: "proj1".into(),
                is_file: false,
                is_directory: true,
                size: None,
            }],
        );
        provider.set_entries(
            "/projects/proj1",
            vec![
                MockDirent {
                    name: "session-keep.jsonl".into(),
                    is_file: true,
                    is_directory: false,
                    size: Some(500),
                },
                MockDirent {
                    name: "session-delete-me.jsonl".into(),
                    is_file: true,
                    is_directory: false,
                    size: Some(800),
                },
            ],
        );
        let mut watcher =
            FileWatcher::with_poll_interval(provider.clone(), TEST_POLL_INTERVAL_MS);
        let mut rx = watcher.receiver();

        watcher
            .start_ssh_polling(std::path::Path::new("/projects"))
            .await
            .unwrap();
        // Wait for baseline poll to complete
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        while rx.try_recv().is_ok() {} // drain baseline (no events)

        // Remove one file — keep only session-keep.jsonl
        provider.set_entries(
            "/projects/proj1",
            vec![MockDirent {
                name: "session-keep.jsonl".into(),
                is_file: true,
                is_directory: false,
                size: Some(500),
            }],
        );

        let event = rx
            .recv()
            .await
            .expect("Should receive Unlink event after file removal");
        assert_eq!(event.event_type, FileChangeType::Unlink);
        assert_eq!(
            event.session_id.as_deref(),
            Some("session-delete-me")
        );

        watcher.stop().await;
    }

    /// Step 4: Overlapping poll guard is a design property.
    ///
    /// `poll_for_changes` checks `poll_in_progress` before running `do_poll`.
    /// However, `do_poll` holds the `SshPollState` Mutex lock for its entire
    /// synchronous body (read_dir, stat — all sync on MockFsProvider), so
    /// there are no await points inside the lock. This makes overlapping polls
    /// structurally impossible — the guard is a safety net for real SSH
    /// implementations where I/O could theoretically yield.
    ///
    /// This test verifies the guard flag is toggled correctly by inspecting
    /// the poll state after starting and stopping a watcher.
    #[tokio::test]
    async fn test_ssh_poll_guard_flag_lifecycle() {
        let provider = Arc::new(MockFsProvider::new("ssh"));
        provider.set_entries("/projects", vec![]);
        let mut watcher =
            FileWatcher::with_poll_interval(provider.clone(), TEST_POLL_INTERVAL_MS);

        // Before watching: guard should be false
        {
            let state = watcher.ssh_poll_state.lock().await;
            assert!(!state.poll_in_progress);
            assert!(!state.primed);
        }

        watcher
            .start_ssh_polling(std::path::Path::new("/projects"))
            .await
            .unwrap();
        // Wait for baseline poll to complete
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        // After baseline: primed=true, poll_in_progress=false (guard released)
        {
            let state = watcher.ssh_poll_state.lock().await;
            assert!(!state.poll_in_progress, "Guard should be released after poll completes");
            assert!(state.primed, "Should be primed after baseline");
        }

        watcher.stop().await;

        // After stop: everything reset
        {
            let state = watcher.ssh_poll_state.lock().await;
            assert!(!state.poll_in_progress);
            assert!(!state.primed);
            assert!(state.polled_file_sizes.is_empty());
        }
    }

    /// Step 5: stop() during active poll.
    ///
    /// Start polling, immediately call stop(). Verify no events received after stop.
    #[tokio::test]
    async fn test_ssh_poll_stop_during_active_poll() {
        let provider = Arc::new(MockFsProvider::new("ssh"));
        provider.set_entries(
            "/projects",
            vec![MockDirent {
                name: "proj1".into(),
                is_file: false,
                is_directory: true,
                size: None,
            }],
        );
        provider.set_entries(
            "/projects/proj1",
            vec![MockDirent {
                name: "session-x.jsonl".into(),
                is_file: true,
                is_directory: false,
                size: Some(100),
            }],
        );
        let mut watcher =
            FileWatcher::with_poll_interval(provider.clone(), TEST_POLL_INTERVAL_MS);
        let mut rx = watcher.receiver();

        watcher
            .start_ssh_polling(std::path::Path::new("/projects"))
            .await
            .unwrap();

        // Stop immediately — the baseline poll may or may not have run yet
        watcher.stop().await;
        assert!(!watcher.is_watching().await);

        // Modify entries after stop to ensure no late events
        provider.set_entries(
            "/projects/proj1",
            vec![MockDirent {
                name: "session-y.jsonl".into(),
                is_file: true,
                is_directory: false,
                size: Some(200),
            }],
        );

        // Wait and verify no events arrive
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let result = rx.try_recv();
        assert!(
            result.is_err(),
            "No events should be received after stop, got: {:?}",
            result.ok()
        );
    }

    /// Step 6: SSH I/O error recovery.
    ///
    /// Make `read_dir` return `Err` for a project path (by not setting entries for it).
    /// Verify no panic, watcher continues. This tests the `Err(_) => continue` path in do_poll.
    #[tokio::test]
    async fn test_ssh_poll_io_error_recovery() {
        let provider = Arc::new(MockFsProvider::new("ssh"));

        // Set entries for /projects but NOT for /projects/proj1
        // This will cause read_dir("/projects/proj1") to return Err
        provider.set_entries(
            "/projects",
            vec![MockDirent {
                name: "proj1".into(),
                is_file: false,
                is_directory: true,
                size: None,
            }],
        );

        let mut watcher =
            FileWatcher::with_poll_interval(provider.clone(), TEST_POLL_INTERVAL_MS);
        let mut rx = watcher.receiver();

        watcher
            .start_ssh_polling(std::path::Path::new("/projects"))
            .await
            .unwrap();

        // Wait for baseline poll to complete (it should not panic despite read_dir error)
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Drain any events (should be none since read_dir for proj1 failed)
        while rx.try_recv().is_ok() {}

        // Now fix the mock — set entries for the project path
        provider.set_entries(
            "/projects/proj1",
            vec![MockDirent {
                name: "session-recovered.jsonl".into(),
                is_file: true,
                is_directory: false,
                size: Some(300),
            }],
        );

        // Next poll should detect the new file
        let event = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            rx.recv(),
        )
        .await
        .expect("Timeout waiting for recovery event")
        .expect("Should receive Add event after recovery");
        assert_eq!(event.event_type, FileChangeType::Add);
        assert_eq!(
            event.session_id.as_deref(),
            Some("session-recovered")
        );

        watcher.stop().await;
    }

    /// Step 7: Context switch (local → SSH → local).
    ///
    /// Create a watcher with local provider, watch, stop.
    /// Create new watcher with SSH provider, watch, verify polling starts. Stop.
    /// Then create another local watcher to verify clean switch back.
    #[tokio::test]
    async fn test_context_switch_local_to_ssh_to_local() {
        // Phase 1: Local mode
        let local_provider = Arc::new(MockFsProvider::new("local"));
        let tmp = std::env::temp_dir().join("file_watcher_test_ctx_switch");
        let _ = std::fs::create_dir_all(&tmp);
        {
            let mut local_watcher = FileWatcher::new(local_provider);
            assert_eq!(local_watcher.mode, WatchMode::Local);
            local_watcher.watch(&tmp).await.unwrap();
            assert!(local_watcher.is_watching().await);
            local_watcher.stop().await;
            assert!(!local_watcher.is_watching().await);
        }

        // Phase 2: SSH mode
        let ssh_provider = Arc::new(MockFsProvider::new("ssh"));
        ssh_provider.set_entries(
            "/ssh-projects",
            vec![MockDirent {
                name: "proj-a".into(),
                is_file: false,
                is_directory: true,
                size: None,
            }],
        );
        ssh_provider.set_entries(
            "/ssh-projects/proj-a",
            vec![MockDirent {
                name: "session-ssh.jsonl".into(),
                is_file: true,
                is_directory: false,
                size: Some(999),
            }],
        );
        {
            let mut ssh_watcher =
                FileWatcher::with_poll_interval(ssh_provider.clone(), TEST_POLL_INTERVAL_MS);
            assert_eq!(ssh_watcher.mode, WatchMode::SshPolling);

            ssh_watcher
                .start_ssh_polling(std::path::Path::new("/ssh-projects"))
                .await
                .unwrap();
            assert!(ssh_watcher.is_watching().await);

            // Wait for baseline, verify no spurious events
            let mut rx = ssh_watcher.receiver();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            assert!(
                rx.try_recv().is_err(),
                "Baseline should produce no events"
            );

            ssh_watcher.stop().await;
            assert!(!ssh_watcher.is_watching().await);
        }

        // Phase 3: Back to local mode — new watcher, clean state
        let local_provider2 = Arc::new(MockFsProvider::new("local"));
        {
            let mut local_watcher2 = FileWatcher::new(local_provider2);
            assert_eq!(local_watcher2.mode, WatchMode::Local);
            assert!(!local_watcher2.is_watching().await);
            // Watch should work on a real temp dir
            local_watcher2.watch(&tmp).await.unwrap();
            assert!(local_watcher2.is_watching().await);
            local_watcher2.stop().await;
            assert!(!local_watcher2.is_watching().await);
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }
}