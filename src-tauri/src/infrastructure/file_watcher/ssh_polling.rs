//! SSH 轮询模式 — 通过 FsProvider 定期扫描文件变更。

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use tokio::sync::Mutex;

use super::{FileWatcher, SshPollState};
use crate::infrastructure::fs_provider::FsProvider;
use crate::types::domain::{FileChangeEvent, FileChangeType};

impl FileWatcher {
    /// 启动 SSH 轮询模式。
    pub(crate) async fn start_ssh_polling(&mut self, path: &Path) -> Result<(), String> {
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
    pub(crate) async fn poll_for_changes(
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
    /// 支持两种目录布局：
    /// - **projects 两层**：`projects/{projectId}/{sessionId}.jsonl`
    /// - **todos 平铺**：`todos/{sessionId}.json`
    ///
    /// 顶层若含 `.json` 文件即视为平铺模式；否则按两层模式枚举 `.jsonl`。
    /// 不递归进入 subagents/ 子目录。
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

        let top_entries = fs_provider
            .read_dir(projects_path)
            .map_err(|e| format!("SSH read_dir failed: {}", e))?;

        // 平铺模式：顶层含 .json 文件 → todos 目录
        let has_top_json = top_entries
            .iter()
            .any(|e| e.is_file && e.name.ends_with(".json"));

        // 枚举要观察的 (path, size) 列表
        let file_entries: Vec<(std::path::PathBuf, u64)> = if has_top_json {
            // todos 平铺模式：直接枚举 .json
            top_entries
                .iter()
                .filter(|e| e.is_file && e.name.ends_with(".json"))
                .map(|e| {
                    let full = projects_path.join(&e.name);
                    let size = e
                        .size
                        .or_else(|| fs_provider.stat(&full).ok().map(|s| s.size))
                        .unwrap_or(0);
                    (full, size)
                })
                .collect()
        } else {
            // projects 两层模式：枚举 project_dir/*.jsonl + project_dir/memory/*.md（Phase 3A）
            let mut acc = Vec::new();
            for dir in &top_entries {
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
                    let size = entry
                        .size
                        .or_else(|| fs_provider.stat(&full_path).ok().map(|s| s.size))
                        .unwrap_or(0);
                    acc.push((full_path, size));
                }

                // Phase 3A: 枚举 project_dir/memory/*.md（MEMORY.md 及其他 .md 文件）
                // 对齐 Electron FileWatcher.ts:490-502 路径模式 projects/<id>/memory/<any>.md
                // TODO(perf): 50 项目 × 3s 间隔会增加 50 次 SFTP readdir。
                //            后续可加 60s TTL memory 目录存在性缓存（plan v3 列入）
                let memory_dir = project_path.join("memory");
                if let Ok(memory_entries) = fs_provider.read_dir(&memory_dir) {
                    for entry in &memory_entries {
                        if !entry.is_file || !entry.name.ends_with(".md") {
                            continue;
                        }
                        let full_path = memory_dir.join(&entry.name);
                        let size = entry
                            .size
                            .or_else(|| fs_provider.stat(&full_path).ok().map(|s| s.size))
                            .unwrap_or(0);
                        acc.push((full_path, size));
                    }
                }
            }
            acc
        };

        // 统一 size 差异检测
        for (full_path, observed_size) in file_entries {
            let path_str = full_path.to_string_lossy().to_string();
            seen_files.insert(path_str.clone());
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
                _ => {}
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
    ///
    /// 支持三种相对路径布局：
    /// - **todos 平铺**：`["{sessionId}.json"]`（1 段 + `.json` 后缀）
    /// - **projects 两层**：`["projectId", "sessionId.jsonl"]`（含可选 subagents）
    /// - **memory**（Phase 3A）：`["projectId", "memory", "{file}.md"]`
    pub(crate) fn emit_event(
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

        // Phase 3A: memory 路径分流
        // projects/<id>/memory/<file>.md → Memory kind
        if parts.len() >= 3 && parts[1] == "memory" && parts[parts.len() - 1].ends_with(".md") {
            let event = FileChangeEvent {
                event_type,
                path: file_path.to_string_lossy().to_string(),
                project_id: Some(parts[0].to_string()),
                session_id: None,
                is_subagent: false,
                kind: crate::types::domain::FileChangeEventKind::Memory,
            };
            let _ = sender.send(event);
            return;
        }

        let (project_id, session_id, is_subagent) = if parts.len() == 1
            && parts[0].ends_with(".json")
        {
            // todos 平铺模式：sessionId = 文件名去后缀，无 project_id
            let sid = std::path::Path::new(parts[0])
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            (None, Some(sid), false)
        } else {
            Self::parse_path_parts(&parts)
        };

        let event = FileChangeEvent {
            event_type,
            path: file_path.to_string_lossy().to_string(),
            project_id,
            session_id,
            is_subagent,
            kind: crate::types::domain::FileChangeEventKind::Session,
        };
        let _ = sender.send(event);
    }
}
