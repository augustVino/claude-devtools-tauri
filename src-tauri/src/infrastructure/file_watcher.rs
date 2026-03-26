//! 文件监听器 — 监听 Claude Code 项目文件的变化。
//!
//! 职责:
//! - 监听目录中会话文件的变更
//! - 对快速连续的文件事件进行防抖（100ms）
//! - 解析路径以提取 projectId、sessionId、isSubagent
//! - 向订阅者广播 FileChangeEvent

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::sync::Mutex;

// 从 notify_debouncer_mini 导入所有 notify 类型，以确保版本兼容性
// (notify-debouncer-mini 使用 notify v7，而直接依赖为 v8)
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, DebouncedEvent};

use crate::types::domain::{FileChangeEvent, FileChangeType};

/// 防抖间隔（毫秒），与 Electron 实现保持一致
const DEBOUNCE_MS: u64 = 100;

/// 文件变更事件的广播通道容量
const CHANNEL_CAPACITY: usize = 64;

/// 文件监听器，以防抖方式监听目录中的文件变更。
pub struct FileWatcher {
    /// 内部防抖监听器（封装以支持异步访问）
    inner: Arc<Mutex<Option<DebouncedWatcher>>>,
    /// 文件变更事件的广播发送端
    sender: broadcast::Sender<FileChangeEvent>,
    /// 是否正在监听
    is_watching: Arc<Mutex<bool>>,
}

/// 防抖监听器句柄的包装结构
struct DebouncedWatcher {
    #[allow(dead_code)]
    watcher: notify_debouncer_mini::Debouncer<RecommendedWatcher>,
}

impl FileWatcher {
    /// 创建新的 FileWatcher 实例。
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(CHANNEL_CAPACITY);
        Self {
            inner: Arc::new(Mutex::new(None)),
            sender,
            is_watching: Arc::new(Mutex::new(false)),
        }
    }

    /// 启动对指定目录的防抖监听。
    ///
    /// 仅对 `.jsonl` 和 `.json` 文件发出事件。
    /// 解析路径以提取 projectId、sessionId 和 isSubagent。
    ///
    /// 路径模式:
    /// - 会话文件: `watchPath/projectId/sessionId.jsonl`
    /// - 子代理文件: `watchPath/projectId/sessionId/subagents/agent-hash.jsonl`
    pub async fn watch(&mut self, path: &Path) -> Result<(), String> {
        if !path.exists() {
            return Err(format!("Path does not exist: {}", path.display()));
        }

        let mut is_watching = self.is_watching.lock().await;
        if *is_watching {
            return Err("Already watching a directory".to_string());
        }

        let sender = self.sender.clone();
        let watch_path = path.to_path_buf();

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
                    Self::process_debounced_event(&debounced_event, &watch_path)
                {
                    let _ = sender.send(change_event);
                }
            }
        });

        *self.inner.lock().await = Some(DebouncedWatcher { watcher: debouncer });
        *is_watching = true;

        log::info!("FileWatcher: Started watching {}", path.display());
        Ok(())
    }

    /// 停止监听目录。
    pub async fn stop(&mut self) {
        let mut inner = self.inner.lock().await;
        let mut is_watching = self.is_watching.lock().await;

        if inner.take().is_some() {
            log::info!("FileWatcher: Stopped watching");
        }
        *is_watching = false;
    }

    /// 返回文件变更事件的广播接收端。
    pub fn receiver(&self) -> broadcast::Receiver<FileChangeEvent> {
        self.sender.subscribe()
    }

    /// 检查当前是否正在监听目录。
    pub async fn is_watching(&self) -> bool {
        *self.is_watching.lock().await
    }

    /// 处理防抖事件并转换为 FileChangeEvent。
    ///
    /// 解析相对于监听根目录的文件路径，提取:
    /// - projectId: 监听路径后的第一级目录
    /// - sessionId: 路径中的会话 ID
    /// - isSubagent: 是否为子代理文件
    ///
    /// 路径模式（与 Electron FileWatcher.ts 一致）:
    /// - 会话文件: `watchPath/projectId/sessionId.jsonl`
    /// - 子代理文件: `watchPath/projectId/sessionId/subagents/agent-hash.jsonl`
    fn process_debounced_event(event: &DebouncedEvent, watch_path: &Path) -> Option<FileChangeEvent> {
        // 仅处理 .jsonl 和 .json 文件
        let extension = event.path.extension()?.to_str()?;

        if extension != "jsonl" && extension != "json" {
            return None;
        }

        // 通过检查文件是否存在来判断是新增/修改还是删除
        // (debouncer-mini 不区分事件类型 — 仅返回 Any/AnyContinuous)
        let event_type = if event.path.exists() {
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
}

impl Default for FileWatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_watcher() {
        let watcher = FileWatcher::new();
        assert!(!watcher.is_watching().await);
    }

    #[tokio::test]
    async fn test_watch_nonexistent_path() {
        let mut watcher = FileWatcher::new();
        let result = watcher.watch(Path::new("/nonexistent/path/12345")).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[tokio::test]
    async fn test_stop_without_watch() {
        let mut watcher = FileWatcher::new();
        // 不应 panic
        watcher.stop().await;
        assert!(!watcher.is_watching().await);
    }

    #[test]
    fn test_receiver_creation() {
        let watcher = FileWatcher::new();
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
}