//! 本地文件监听 — 使用 notify_debouncer_mini 监听 OS 级文件事件。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify_debouncer_mini::notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebouncedEvent};

use super::{DebouncedWatcher, FileWatcher, DEBOUNCE_MS};
use crate::infrastructure::fs_provider::FsProvider;
use crate::types::domain::{FileChangeEvent, FileChangeType};

impl FileWatcher {
    /// 本地模式：使用 notify_debouncer_mini 监听多个根的文件变更。
    ///
    /// `paths[0]` 是主根（claude projects_dir），用于 claude 布局的相对路径
    /// 解析（projectId/sessionId/memory 分流）；其余根（pi/codex 等 extra
    /// agent）由 `agents::adapter_for_path` 结构特征分派解析。单根失败
    /// （不存在等）warn 跳过；全部失败才返回 Err。
    pub(crate) async fn watch_local(&mut self, paths: &[PathBuf]) -> Result<(), String> {
        let mut is_watching = self.is_watching.lock().await;
        if *is_watching {
            return Err("Already watching a directory".to_string());
        }

        // 过滤出存在的根；全失败才报错（单 agent 未安装不能拖垮监听）
        let watchable: Vec<PathBuf> = paths
            .iter()
            .filter(|p| self.fs_provider.exists(p).unwrap_or(false))
            .cloned()
            .collect();
        if watchable.is_empty() {
            return Err(format!(
                "No watchable paths among: {}",
                paths.iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for skipped in paths.iter().filter(|p| !watchable.contains(p)) {
            log::info!("FileWatcher: skip missing watch root {}", skipped.display());
        }

        let sender = self.sender.clone();
        let claude_root = watchable[0].clone();
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

        for root in &watchable {
            debouncer
                .watcher()
                .watch(root, RecursiveMode::Recursive)
                .map_err(|e| format!("Failed to watch {}: {}", root.display(), e))?;
        }

        // 启动异步任务处理防抖后的事件
        tokio::spawn(async move {
            while let Some(debounced_event) = rx.recv().await {
                if let Some(change_event) = Self::process_debounced_event_with_provider(
                    &fs_provider,
                    &debounced_event,
                    &claude_root,
                ) {
                    let _ = sender.send(change_event);
                }
            }
        });

        *self.local_watcher.lock().await = Some(DebouncedWatcher { watcher: debouncer });
        *is_watching = true;

        log::info!(
            "FileWatcher: Started local watching {} roots",
            watchable.len()
        );
        Ok(())
    }

    /// 处理防抖事件并转换为 FileChangeEvent（使用 FsProvider 检查文件存在性）。
    ///
    /// 解析分派（按路径结构特征，本地与 SSH 远端通用）：
    /// - **extra agent 根**（`.pi/agent/sessions` / `.codex/{,archived_}sessions`）：
    ///   `agents::adapter_for_path` → `resolve_watch_event`（读文件头拿
    ///   session_id + cwd）→ projectId = encode_path(cwd)（供 orchestrator
    ///   缓存失效；发前端前 orchestrator 会置 None）；
    /// - **claude 根**（`claude_root` 前缀）：相对路径 parts 解析 ——
    ///   会话文件 `projectId/sessionId.jsonl`、子代理
    ///   `projectId/sessionId/subagents/agent-hash.jsonl`、memory
    ///   `projectId/memory/*.md`。
    pub(crate) fn process_debounced_event_with_provider(
        fs_provider: &Arc<dyn FsProvider>,
        event: &DebouncedEvent,
        claude_root: &Path,
    ) -> Option<FileChangeEvent> {
        // Phase 3A: 接受 .jsonl / .json / .md（memory 目录下的 MEMORY.md 等）
        let extension = event.path.extension()?.to_str()?;
        let is_md = extension == "md";
        if extension != "jsonl" && extension != "json" && !is_md {
            return None;
        }

        // 通过 FsProvider 检查文件是否存在来判断是新增/修改还是删除
        let event_type = if fs_provider.exists(&event.path).unwrap_or(false) {
            FileChangeType::Change
        } else {
            FileChangeType::Unlink
        };

        // extra agent 分派：结构特征命中（pi/codex）→ 读头解析
        // （读头失败 = 半写/边车 → 静默丢弃，下轮事件重试）
        let adapter = crate::agents::adapter_for_path(&event.path);
        if adapter.kind() != crate::types::domain::AgentKind::ClaudeCode {
            let (session_id, cwd) = adapter.resolve_watch_event(&event.path, fs_provider.as_ref())?;
            return Some(FileChangeEvent {
                event_type,
                path: event.path.to_string_lossy().to_string(),
                project_id: Some(crate::utils::encode_path(&cwd)),
                session_id: Some(session_id),
                is_subagent: false,
                kind: crate::types::domain::FileChangeEventKind::Session,
                agent: Some(adapter.kind()),
            });
        }

        // claude 布局：相对主根解析
        let relative_path = event.path.strip_prefix(claude_root).ok()?;
        let parts: Vec<&str> = relative_path
            .components()
            .filter_map(|c| c.as_os_str().to_str())
            .collect();

        // Phase 3A: MEMORY.md 分流 — parts[1] == "memory" 且扩展名 .md
        // 对齐 Electron FileWatcher.ts:490-502 handleProjectsChange 路径判断
        if is_md && parts.len() >= 2 && parts[1] == "memory" {
            return Some(FileChangeEvent {
                event_type,
                path: event.path.to_string_lossy().to_string(),
                project_id: Some(parts[0].to_string()),
                session_id: None,
                is_subagent: false,
                kind: crate::types::domain::FileChangeEventKind::Memory,
                agent: None,
            });
        }

        // 非 memory 的 .md 不处理
        if is_md {
            return None;
        }

        let (project_id, session_id, is_subagent) = Self::parse_path_parts(&parts);

        Some(FileChangeEvent {
            event_type,
            path: event.path.to_string_lossy().to_string(),
            project_id,
            session_id,
            is_subagent,
            kind: crate::types::domain::FileChangeEventKind::Session,
            agent: None,
        })
    }
}
