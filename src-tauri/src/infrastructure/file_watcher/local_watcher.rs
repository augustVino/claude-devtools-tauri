//! 本地文件监听 — 使用 notify_debouncer_mini 监听 OS 级文件事件。

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use notify_debouncer_mini::{new_debouncer, DebouncedEvent};
use notify_debouncer_mini::notify::RecursiveMode;

use super::{DebouncedWatcher, DEBOUNCE_MS, FileWatcher};
use crate::infrastructure::fs_provider::FsProvider;
use crate::types::domain::{FileChangeEvent, FileChangeType};

impl FileWatcher {
    /// 本地模式：使用 notify_debouncer_mini 监听文件变更。
    pub(crate) async fn watch_local(&mut self, path: &Path) -> Result<(), String> {
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
    pub(crate) fn process_debounced_event_with_provider(
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
}
