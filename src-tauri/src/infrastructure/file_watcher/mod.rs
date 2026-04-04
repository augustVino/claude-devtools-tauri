//! 文件监听器 — 监听 Claude Code 项目文件的变化。
//!
//! 职责:
//! - 监听目录中会话文件的变更
//! - 对快速连续的文件事件进行防抖（100ms）
//! - 解析路径以提取 projectId、sessionId、isSubagent
//! - 向订阅者广播 FileChangeEvent

pub mod local_watcher;
pub mod path_parser;
pub mod ssh_polling;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
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
pub(crate) const SSH_POLL_INTERVAL_MS: u64 = 3000;

/// 测试用轮询间隔（毫秒），避免测试等待 3 秒
#[cfg(test)]
pub(crate) const TEST_POLL_INTERVAL_MS: u64 = 50;

/// 文件监听模式。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum WatchMode {
    /// 本地模式：使用 notify_debouncer_mini (OS 级文件事件)
    Local,
    /// SSH 轮询模式：通过 FsProvider 定期扫描文件变更
    SshPolling,
}

/// SSH 轮询状态。
pub(crate) struct SshPollState {
    /// 轮询任务句柄（Option: 无轮询时为 None）
    pub timer: Option<JoinHandle<()>>,
    /// 轮询间隔（毫秒）
    pub poll_interval_ms: u64,
    /// 首次基线扫描是否已完成
    pub primed: bool,
    /// 已追踪的文件大小 {绝对路径 -> 字节数}
    pub polled_file_sizes: HashMap<String, u64>,
    /// 防止重叠轮询的 guard
    pub poll_in_progress: bool,
}

/// 文件监听器，以防抖方式监听目录中的文件变更。
pub struct FileWatcher {
    /// 监听模式（本地 or SSH 轮询）
    pub(crate) mode: WatchMode,
    /// 文件系统提供者
    fs_provider: Arc<dyn FsProvider>,
    /// 文件变更事件的广播发送端
    sender: broadcast::Sender<FileChangeEvent>,
    /// 是否正在监听
    is_watching: Arc<Mutex<bool>>,
    /// 本地模式：防抖监听器
    local_watcher: Arc<Mutex<Option<DebouncedWatcher>>>,
    /// SSH 模式：轮询状态
    pub(crate) ssh_poll_state: Arc<Mutex<SshPollState>>,
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
}

impl Default for FileWatcher {
    fn default() -> Self {
        Self::new(Arc::new(crate::infrastructure::fs_provider::LocalFsProvider::new()))
    }
}
