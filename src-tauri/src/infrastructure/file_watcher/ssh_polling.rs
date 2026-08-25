//! SSH 轮询模式 — 通过 FsProvider 定期扫描文件变更。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;
use tokio::sync::Mutex;

use super::{FileWatcher, SshPollState};
use crate::infrastructure::fs_provider::FsProvider;
use crate::types::domain::{FileChangeEvent, FileChangeType};

impl FileWatcher {
    /// 启动 SSH 轮询模式（多根）。
    ///
    /// `paths[0]` 为主根（claude projects_dir / todos_dir），其余为 extra
    /// agent 根（两层布局；codex 日期树由 `agents::watch_roots` 排除）。
    pub(crate) async fn start_ssh_polling(&mut self, paths: &[PathBuf]) -> Result<(), String> {
        let watchable: Vec<PathBuf> = paths
            .iter()
            .filter(|p| self.fs_provider.exists(p).unwrap_or(false))
            .cloned()
            .collect();
        if watchable.is_empty() {
            return Err(format!(
                "No watchable paths (SSH) among: {}",
                paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
            ));
        }

        let mut is_watching = self.is_watching.lock().await;
        if *is_watching {
            return Err("Already watching a directory".to_string());
        }

        let sender = self.sender.clone();
        let fs_provider = self.fs_provider.clone();
        let poll_state = self.ssh_poll_state.clone();
        let roots = watchable;

        // 读取间隔（允许测试覆盖）
        let poll_interval = self.ssh_poll_state.lock().await.poll_interval_ms;

        // 重置状态以确保全新开始
        {
            let mut state = poll_state.lock().await;
            state.primed = false;
            state.polled_file_sizes.clear();
            state.poll_in_progress = false;
        }

        let roots_len = roots.len();
        let handle = tokio::spawn(async move {
            // 立即执行首次基线扫描
            Self::poll_for_changes(&fs_provider, &roots, &poll_state, &sender).await;

            loop {
                tokio::time::sleep(Duration::from_millis(poll_interval)).await;
                Self::poll_for_changes(&fs_provider, &roots, &poll_state, &sender).await;
            }
        });

        self.ssh_poll_state.lock().await.timer = Some(handle);
        *is_watching = true;

        log::info!(
            "FileWatcher: Started SSH polling {} roots (interval={}ms)",
            roots_len,
            poll_interval
        );
        Ok(())
    }

    /// 执行一次 SSH 轮询扫描。
    pub(crate) async fn poll_for_changes(
        fs_provider: &Arc<dyn FsProvider>,
        roots: &[PathBuf],
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

        let result = Self::do_poll(fs_provider, roots, poll_state, sender).await;

        poll_state.lock().await.poll_in_progress = false;

        if let Err(e) = result {
            log::error!("SSH poll error: {}", e);
        }
    }

    /// 实际的轮询逻辑（多根）。
    ///
    /// 每个根支持两种目录布局：
    /// - **projects 两层**：`projects/{projectId}/{sessionId}.jsonl`（claude
    ///   与 pi 同构；extra agent 的解析在 emit 阶段按路径特征分派）
    /// - **todos 平铺**：`todos/{sessionId}.json`
    ///
    /// 单根枚举失败只跳过该根（warn），全部失败才向上报错。
    pub(crate) async fn do_poll(
        fs_provider: &Arc<dyn FsProvider>,
        roots: &[PathBuf],
        poll_state: &Arc<Mutex<SshPollState>>,
        sender: &broadcast::Sender<FileChangeEvent>,
    ) -> Result<(), String> {
        let mut state = poll_state.lock().await;
        let primed = state.primed;
        let mut seen_files = HashSet::new();
        let mut pending_events: Vec<(std::path::PathBuf, FileChangeType)> = Vec::new();

        let mut file_entries: Vec<(std::path::PathBuf, u64)> = Vec::new();
        let mut failed_roots = 0usize;
        for root in roots {
            match Self::enumerate_root_files(fs_provider, root, &mut state) {
                Ok(entries) => file_entries.extend(entries),
                Err(e) => {
                    failed_roots += 1;
                    log::warn!("SSH poll root {} failed: {}", root.display(), e);
                }
            }
        }
        if failed_roots == roots.len() {
            return Err("SSH read_dir failed for all roots".to_string());
        }

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
            Self::emit_event(sender, &path, &roots[0], fs_provider, event_type);
        }

        Ok(())
    }

    /// 单根文件枚举（同步）：平铺（todos .json）/ 两层（claude·pi .jsonl + memory .md）。
    /// 布局判定：顶层含 .json 文件 → 平铺；否则两层。memory 枚举带 60s TTL
    /// absent 缓存（review #7，细节见原注释）。
    fn enumerate_root_files(
        fs_provider: &Arc<dyn FsProvider>,
        root: &Path,
        state: &mut SshPollState,
    ) -> Result<Vec<(std::path::PathBuf, u64)>, String> {

        let top_entries = fs_provider
            .read_dir(root)
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
                    let full = root.join(&e.name);
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
                let project_path = root.join(&dir.name);
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

                // Phase 3A + review #7: 枚举 project_dir/memory/*.md（带 60s TTL 缓存）
                // 路径模式：projects/<id>/memory/<any>.md
                // 性能：50 项目 × 3s 间隔时，对不存在 memory 目录的 project 用 cache 跳过 readdir，
                // 避免每秒 ~17 次 SFTP readdir（慢速 SSH 主机会显著拖累 polling）。
                // 注意：本 polling 是超出 Electron 行为的扩展（Electron SSH 不检测 memory）。
                // 行为权衡：cache 命中 absent 时，新创建 memory 文件最多 60s 后才发现。
                let memory_dir = project_path.join("memory");
                let memory_dir_key = memory_dir.to_string_lossy().to_string();
                let should_skip = matches!(
                    state.memory_dir_cache.get(&memory_dir_key),
                    Some(entry) if entry.is_fresh() && !entry.exists
                );
                if !should_skip {
                    match fs_provider.read_dir(&memory_dir) {
                        Ok(memory_entries) => {
                            state.memory_dir_cache.insert(
                                memory_dir_key.clone(),
                                super::MemoryDirCacheEntry {
                                    exists: true,
                                    last_check: std::time::Instant::now(),
                                },
                            );
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
                        Err(err_msg) => {
                            // review #4: 只在 Err 消息包含 POSIX "no such file" / "not a directory"
                            // 或 russh-sftp "NoSuchFile" / "NotADirectory" 时标记 absent。
                            // 瞬时网络 Err（timeout/IO）→ 不更新 cache，下一轮重试。
                            let lower = err_msg.to_lowercase();
                            let is_permanent_absent = lower.contains("no such file")
                                || lower.contains("not a directory")
                                || lower.contains("nosuchfile")
                                || lower.contains("notadirectory");
                            if is_permanent_absent {
                                state.memory_dir_cache.insert(
                                    memory_dir_key,
                                    super::MemoryDirCacheEntry {
                                        exists: false,
                                        last_check: std::time::Instant::now(),
                                    },
                                );
                            }
                        }
                    }
                }
            }
            acc
        };

        Ok(file_entries)
    }


    /// 构造并发送 FileChangeEvent。
    ///
    /// 解析分派：
    /// - **extra agent**（pi 等两层布局根）：结构特征命中 → 读头解析
    ///   （session_id + cwd），projectId = encode_path(cwd)，agent = Some；
    /// - **claude/todos**：相对主根布局解析（支持 todos 平铺 / projects 两层
    ///   / memory 三种，见下）。
    pub(crate) fn emit_event(
        sender: &broadcast::Sender<FileChangeEvent>,
        file_path: &Path,
        projects_path: &Path,
        fs_provider: &Arc<dyn FsProvider>,
        event_type: FileChangeType,
    ) {
        // extra agent 分派（与 local process_debounced_event 同一语义）。
        // 读头失败 = 半写/边车 → 静默丢弃，下轮轮询重试。
        let adapter = crate::agents::adapter_for_path(file_path);
        if adapter.kind() != crate::types::domain::AgentKind::ClaudeCode {
            if let Some((session_id, cwd)) = adapter.resolve_watch_event(file_path, fs_provider.as_ref()) {
                let event = FileChangeEvent {
                    event_type,
                    path: file_path.to_string_lossy().to_string(),
                    project_id: Some(crate::utils::encode_path(&cwd)),
                    session_id: Some(session_id),
                    is_subagent: false,
                    kind: crate::types::domain::FileChangeEventKind::Session,
                    agent: Some(adapter.kind()),
                };
                let _ = sender.send(event);
            }
            return;
        }

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
                agent: None,
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
            agent: None,
        };
        let _ = sender.send(event);
    }
}
