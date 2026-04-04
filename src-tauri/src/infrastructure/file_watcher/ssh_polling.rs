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
