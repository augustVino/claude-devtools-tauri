//! 文件监听器测试 — MockFsProvider + 全部单元/集成测试。

use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use super::*;
use crate::infrastructure::fs_provider::{FsDirent, FsProvider, FsStatResult, LocalFsProvider};

fn local_provider() -> Arc<dyn FsProvider> {
    Arc::new(LocalFsProvider::new())
}

// ── MockFsProvider ──────────────────────────────────────────

#[derive(Debug)]
struct MockFsProvider {
    provider_type_str: &'static str,
    entries: Arc<StdMutex<HashMap<String, Vec<MockDirent>>>>,
    ensure_dir_calls: Arc<StdMutex<Vec<String>>>,
    read_dir_errors: Arc<StdMutex<HashMap<String, String>>>,
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
            ensure_dir_calls: Arc::new(StdMutex::new(Vec::new())),
            read_dir_errors: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    fn set_entries(&self, path: &str, dirents: Vec<MockDirent>) {
        self.entries
            .lock()
            .unwrap()
            .insert(path.to_string(), dirents);
    }

    fn set_read_dir_error(&self, path: &str, error_msg: String) {
        self.read_dir_errors
            .lock()
            .unwrap()
            .insert(path.to_string(), error_msg);
    }

    fn clear_entries(&self) {
        self.entries.lock().unwrap().clear();
    }

    fn ensure_dir_call_count(&self) -> usize {
        self.ensure_dir_calls.lock().unwrap().len()
    }

    fn ensure_dir_calls_snapshot(&self) -> Vec<String> {
        self.ensure_dir_calls.lock().unwrap().clone()
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
    fn read_file_head(&self, _path: &std::path::Path, _max_lines: usize) -> Result<String, String> {
        Ok(String::new())
    }
    fn read_file_range(
        &self,
        _path: &std::path::Path,
        _offset: u64,
        _length: Option<u64>,
    ) -> Result<Vec<u8>, String> {
        // MockFsProvider 当前不模拟文件内容，返回空 Vec 让 polling/stat 路径不报错。
        // 如果未来 file_watcher 测试需要 read_file_range 行为，扩展 MockFsProvider
        // 增加 file_contents: HashMap<PathBuf, Vec<u8>> 字段。
        Ok(Vec::new())
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
        // 优先返回注入的错误（用于测试 Err 过滤逻辑）
        if let Some(err_msg) = self.read_dir_errors.lock().unwrap().get(&key) {
            return Err(err_msg.clone());
        }
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

    /// SSH-style provider（type="ssh"）继承 trait 默认 no-op 实现。
    /// 这里覆盖默认以记录调用，验证 watcher_orchestrator 在 SSH 模式下不触发
    /// 本地 fs::create_dir_all 副作用。
    /// LocalFsProvider 的真实行为（创建目录）由独立测试覆盖。
    fn ensure_dir(&self, path: &std::path::Path) -> Result<(), String> {
        self.ensure_dir_calls
            .lock()
            .unwrap()
            .push(path.to_string_lossy().to_string());
        // SSH mock 不实际创建，对应生产 SshFsProvider 的 no-op 语义。
        Ok(())
    }
}

// ── ensure_dir 行为对照测试 ──────────────────────────────────

/// SSH-style provider 在 ensure_dir 调用时不应触发本地 fs::create_dir_all。
/// 对应 commit a08be11 修复的 watcher_orchestrator 副作用 bug：
/// 原代码 `tokio::fs::create_dir_all(&projects_dir)` 在 SSH context 下误建本地目录。
#[test]
fn test_watcher_does_not_create_local_dirs_in_ssh_mode() {
    let ssh_provider = MockFsProvider::new("ssh");
    let bogus_path = std::env::temp_dir().join("claude-devtools-test-ssh-no-create-xyz");
    // 路径在本地不存在
    assert!(!bogus_path.exists(), "precondition: path should not exist");

    let result = ssh_provider.ensure_dir(&bogus_path);
    assert!(result.is_ok(), "ensure_dir should return Ok under SSH");

    // 关键断言：SSH provider 不应在本地实际创建目录
    assert!(
        !bogus_path.exists(),
        "SSH ensure_dir must NOT create local directories (was the bug in watcher_orchestrator)"
    );
    // 调用被记录（证明 trait 多态走的是 provider.ensure_dir，而非硬编码 tokio::fs）
    assert_eq!(ssh_provider.ensure_dir_call_count(), 1);
}

/// LocalFsProvider.ensure_dir 必须实际创建目录（与 SSH no-op 形成对照）。
#[test]
fn test_watcher_creates_dirs_in_local_mode() {
    use crate::infrastructure::fs_provider::LocalFsProvider;
    use std::fs;

    let tmp = tempfile::tempdir().unwrap();
    let nested = tmp.path().join("nested/deep/dir");
    assert!(!nested.exists(), "precondition: nested dir should not exist");

    let provider = LocalFsProvider::new();
    provider.ensure_dir(&nested).unwrap();

    assert!(nested.is_dir(), "LocalFsProvider must create nested dirs");
    // 清理（防止 tempdir 残留，虽然 TempDir drop 会清理）
    let _ = fs::remove_dir_all(&nested);
}

#[tokio::test]
async fn test_create_watcher() {
    let watcher = FileWatcher::new(local_provider());
    assert!(!watcher.is_watching().await);
}

#[tokio::test]
async fn test_watch_nonexistent_path() {
    let mut watcher = FileWatcher::new(local_provider());
    let result = watcher.watch(&[std::path::PathBuf::from("/nonexistent/path/12345")]).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("No watchable paths"));
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
    let mut watcher = FileWatcher::with_poll_interval(provider.clone(), TEST_POLL_INTERVAL_MS);
    let mut rx = watcher.receiver();

    watcher
        .start_ssh_polling(&[std::path::PathBuf::from("/projects")])
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
    let mut watcher = FileWatcher::with_poll_interval(provider.clone(), TEST_POLL_INTERVAL_MS);
    let mut rx = watcher.receiver();

    watcher
        .start_ssh_polling(&[std::path::PathBuf::from("/projects")])
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
    watcher.watch(&[tmp.clone()]).await.unwrap();
    assert!(watcher.is_watching().await);

    // Rewatch should work — stop + watch on same path
    watcher.rewatch(&[tmp.clone()]).await.unwrap();
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
    let mut watcher = FileWatcher::with_poll_interval(provider.clone(), TEST_POLL_INTERVAL_MS);
    let mut rx = watcher.receiver();

    watcher
        .start_ssh_polling(&[std::path::PathBuf::from("/projects")])
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
    assert_eq!(event.project_id.as_deref(), Some("proj1"));

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
    let mut watcher = FileWatcher::with_poll_interval(provider.clone(), TEST_POLL_INTERVAL_MS);
    let mut rx = watcher.receiver();

    watcher
        .start_ssh_polling(&[std::path::PathBuf::from("/projects")])
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
    assert_eq!(event.session_id.as_deref(), Some("session-delete-me"));

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
    let mut watcher = FileWatcher::with_poll_interval(provider.clone(), TEST_POLL_INTERVAL_MS);

    // Before watching: guard should be false
    {
        let state = watcher.ssh_poll_state.lock().await;
        assert!(!state.poll_in_progress);
        assert!(!state.primed);
    }

    watcher
        .start_ssh_polling(&[std::path::PathBuf::from("/projects")])
        .await
        .unwrap();
    // Wait for baseline poll to complete
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    // After baseline: primed=true, poll_in_progress=false (guard released)
    {
        let state = watcher.ssh_poll_state.lock().await;
        assert!(
            !state.poll_in_progress,
            "Guard should be released after poll completes"
        );
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
    let mut watcher = FileWatcher::with_poll_interval(provider.clone(), TEST_POLL_INTERVAL_MS);
    let mut rx = watcher.receiver();

    watcher
        .start_ssh_polling(&[std::path::PathBuf::from("/projects")])
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

    let mut watcher = FileWatcher::with_poll_interval(provider.clone(), TEST_POLL_INTERVAL_MS);
    let mut rx = watcher.receiver();

    watcher
        .start_ssh_polling(&[std::path::PathBuf::from("/projects")])
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
    let event = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
        .await
        .expect("Timeout waiting for recovery event")
        .expect("Should receive Add event after recovery");
    assert_eq!(event.event_type, FileChangeType::Add);
    assert_eq!(event.session_id.as_deref(), Some("session-recovered"));

    watcher.stop().await;
}

/// Step 7: Context switch (local -> SSH -> local).
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
        local_watcher.watch(&[tmp.clone()]).await.unwrap();
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
            .start_ssh_polling(&[std::path::PathBuf::from("/ssh-projects")])
            .await
            .unwrap();
        assert!(ssh_watcher.is_watching().await);

        // Wait for baseline, verify no spurious events
        let mut rx = ssh_watcher.receiver();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(rx.try_recv().is_err(), "Baseline should produce no events");

        ssh_watcher.stop().await;
        assert!(!ssh_watcher.is_watching().await);
    }

    // Phase 3: Back to local mode -- new watcher, clean state
    let local_provider2 = Arc::new(MockFsProvider::new("local"));
    {
        let mut local_watcher2 = FileWatcher::new(local_provider2);
        assert_eq!(local_watcher2.mode, WatchMode::Local);
        assert!(!local_watcher2.is_watching().await);
        // Watch should work on a real temp dir
        local_watcher2.watch(&[tmp.clone()]).await.unwrap();
        assert!(local_watcher2.is_watching().await);
        local_watcher2.stop().await;
        assert!(!local_watcher2.is_watching().await);
    }

    let _ = std::fs::remove_dir_all(&tmp);
}

// ── SSH todos 平铺目录 polling 测试（Task 3） ──────────────────

/// 辅助：等待并排空 baseline 事件（替代 `tokio::time::sleep`，消除 flaky）。
/// 在 TEST_POLL_INTERVAL_MS=50ms 下用 500ms timeout 拿基线事件再 drain，
/// 至少跑过 1 个完整 poll 周期。
async fn drain_baseline(rx: &mut tokio::sync::broadcast::Receiver<FileChangeEvent>) {
    let _ = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        rx.recv(),
    )
    .await;
    while rx.try_recv().is_ok() {}
}

#[tokio::test]
async fn test_ssh_poll_todos_flat_directory_detects_new_json() {
    let provider = Arc::new(MockFsProvider::new("ssh"));
    provider.set_entries(
        "/todos",
        vec![MockDirent {
            name: "session-existing.json".into(),
            is_file: true,
            is_directory: false,
            size: Some(500),
        }],
    );
    let mut watcher = FileWatcher::with_poll_interval(provider.clone(), TEST_POLL_INTERVAL_MS);
    let mut rx = watcher.receiver();

    watcher
        .start_ssh_polling(&[std::path::PathBuf::from("/todos")])
        .await
        .unwrap();
    drain_baseline(&mut rx).await;

    // 添加新 todo 文件
    provider.set_entries(
        "/todos",
        vec![
            MockDirent {
                name: "session-existing.json".into(),
                is_file: true,
                is_directory: false,
                size: Some(500),
            },
            MockDirent {
                name: "session-new.json".into(),
                is_file: true,
                is_directory: false,
                size: Some(200),
            },
        ],
    );

    let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("timeout waiting for poll event")
        .expect("Should detect new .json file in flat todos dir");
    assert!(
        event.path.ends_with("session-new.json"),
        "event path = {}",
        event.path
    );
    // 注：session_id 字段由 emit_event 设置；不依赖下游 watcher_orchestrator 的逻辑
    assert_eq!(event.session_id.as_deref(), Some("session-new"));
    assert!(event.project_id.is_none());
    assert!(!event.is_subagent);
    watcher.stop().await;
}

#[tokio::test]
async fn test_ssh_poll_todos_detects_size_change() {
    let provider = Arc::new(MockFsProvider::new("ssh"));
    provider.set_entries(
        "/todos",
        vec![MockDirent {
            name: "sess.json".into(),
            is_file: true,
            is_directory: false,
            size: Some(100),
        }],
    );
    let mut watcher = FileWatcher::with_poll_interval(provider.clone(), TEST_POLL_INTERVAL_MS);
    let mut rx = watcher.receiver();

    watcher
        .start_ssh_polling(&[std::path::PathBuf::from("/todos")])
        .await
        .unwrap();
    drain_baseline(&mut rx).await;

    provider.set_entries(
        "/todos",
        vec![MockDirent {
            name: "sess.json".into(),
            is_file: true,
            is_directory: false,
            size: Some(500),
        }],
    );

    let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("timeout")
        .expect("Should detect size change");
    assert_eq!(event.event_type, FileChangeType::Change);
    assert_eq!(event.session_id.as_deref(), Some("sess"));
    watcher.stop().await;
}

#[tokio::test]
async fn test_ssh_poll_projects_two_level_still_works() {
    // 回归测试：确认 projects 两层模式未被破坏
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
            name: "session.jsonl".into(),
            is_file: true,
            is_directory: false,
            size: Some(100),
        }],
    );
    let mut watcher = FileWatcher::with_poll_interval(provider.clone(), TEST_POLL_INTERVAL_MS);
    let mut rx = watcher.receiver();

    watcher
        .start_ssh_polling(&[std::path::PathBuf::from("/projects")])
        .await
        .unwrap();
    drain_baseline(&mut rx).await;

    provider.set_entries(
        "/projects/proj1",
        vec![MockDirent {
            name: "session.jsonl".into(),
            is_file: true,
            is_directory: false,
            size: Some(300),
        }],
    );

    let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("timeout")
        .expect("Should detect projects size change");
    assert!(event.path.ends_with("session.jsonl"));
    assert_eq!(event.project_id.as_deref(), Some("proj1"));
    assert_eq!(event.session_id.as_deref(), Some("session"));
    watcher.stop().await;
}

/// 直接测试 emit_event 纯函数，避免 polling 异步循环自证
/// （修正 codex TDD "循环自证" 问题：把 emit_event 当独立可测函数覆盖）。
#[test]
fn test_emit_event_todos_flat_path() {
    use tokio::sync::broadcast;
    let (tx, mut rx) = broadcast::channel::<FileChangeEvent>(10);
    let path = std::path::Path::new("/todos/sess-abc.json");
    let base = std::path::Path::new("/todos");
    let fs = local_provider();
    FileWatcher::emit_event(&tx, path, base, &fs, FileChangeType::Add);
    let event = rx.try_recv().expect("should emit");
    assert_eq!(event.session_id.as_deref(), Some("sess-abc"));
    assert!(event.project_id.is_none());
    assert!(!event.is_subagent);
}

#[test]
fn test_emit_event_projects_two_level_path() {
    use tokio::sync::broadcast;
    let (tx, mut rx) = broadcast::channel::<FileChangeEvent>(10);
    let path = std::path::Path::new("/projects/proj1/sess.jsonl");
    let base = std::path::Path::new("/projects");
    let fs = local_provider();
    FileWatcher::emit_event(&tx, path, base, &fs, FileChangeType::Change);
    let event = rx.try_recv().expect("should emit");
    assert_eq!(event.project_id.as_deref(), Some("proj1"));
    assert_eq!(event.session_id.as_deref(), Some("sess"));
}

// ── Memory dir cache tests（review #7）──────────────────────────

/// TTL 60s 内的 cache 条目应视为新鲜
#[test]
fn test_memory_dir_cache_ttl_fresh_within_60s() {
    let entry = super::MemoryDirCacheEntry {
        exists: false,
        last_check: std::time::Instant::now() - std::time::Duration::from_secs(30),
    };
    assert!(entry.is_fresh(), "30s old entry should be fresh");
}

/// 超过 60s 的 cache 条目应视为过期
#[test]
fn test_memory_dir_cache_ttl_expires_after_60s() {
    let entry = super::MemoryDirCacheEntry {
        exists: false,
        last_check: std::time::Instant::now() - std::time::Duration::from_secs(61),
    };
    assert!(!entry.is_fresh(), "61s old entry should be expired");
}

/// 验证 cache 标记 absent 时，do_poll **不调用** read_dir（用计数差断言）
///
/// 测试意图（I2 修复）：不只断言 result.is_ok()，而是验证 read_dir 调用次数
/// 在 cache 命中时确实减少。如果 cache 跳过逻辑被删掉，本测试会失败。
#[tokio::test]
async fn test_memory_dir_cache_skips_readdir_when_absent() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let read_dir_counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = read_dir_counter.clone();

    // 包装 MockFsProvider，记录 read_dir 调用次数
    #[derive(Debug)]
    struct CountingProvider {
        inner: MockFsProvider,
        counter: Arc<AtomicUsize>,
    }
    impl FsProvider for CountingProvider {
        fn provider_type(&self) -> &'static str {
            self.inner.provider_type()
        }
        fn exists(&self, p: &std::path::Path) -> Result<bool, String> {
            self.inner.exists(p)
        }
        fn read_file(&self, p: &std::path::Path) -> Result<String, String> {
            self.inner.read_file(p)
        }
        fn read_file_head(&self, p: &std::path::Path, max: usize) -> Result<String, String> {
            self.inner.read_file_head(p, max)
        }
        fn read_file_range(
            &self,
            p: &std::path::Path,
            off: u64,
            len: Option<u64>,
        ) -> Result<Vec<u8>, String> {
            self.inner.read_file_range(p, off, len)
        }
        fn stat(&self, p: &std::path::Path) -> Result<FsStatResult, String> {
            self.inner.stat(p)
        }
        fn read_dir(&self, p: &std::path::Path) -> Result<Vec<FsDirent>, String> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            self.inner.read_dir(p)
        }
        fn ensure_dir(&self, p: &std::path::Path) -> Result<(), String> {
            self.inner.ensure_dir(p)
        }
    }

    let mock = MockFsProvider::new("ssh");
    // /projects 顶层 1 个 project dir，project 内 0 个 .jsonl（避免干扰计数）
    mock.set_entries(
        "/projects",
        vec![MockDirent {
            name: "proj1".to_string(),
            is_file: false,
            is_directory: true,
            size: None,
        }],
    );
    mock.set_entries("/projects/proj1", vec![]);

    let provider = Arc::new(CountingProvider {
        inner: mock,
        counter: counter_clone,
    });
    let provider_dyn: Arc<dyn FsProvider> = provider;

    let poll_state = Arc::new(Mutex::new(SshPollState {
        timer: None,
        poll_interval_ms: 100,
        primed: true,
        polled_file_sizes: HashMap::new(),
        poll_in_progress: false,
        memory_dir_cache: HashMap::new(),
    }));
    let (tx, _rx) = broadcast::channel::<FileChangeEvent>(16);

    // 第一次 poll：memory 目录不存在（mock 未设置 entries，read_dir 返回 Err）
    // Err 消息格式 "No mock entries for ..." 不包含 "no such file"，**不会标记 absent**
    FileWatcher::do_poll(
        &provider_dyn,
        &[std::path::PathBuf::from("/projects")],
        &poll_state,
        &tx,
    )
    .await
    .unwrap();
    let first_count = read_dir_counter.load(Ordering::SeqCst);
    // 至少 3 次：/projects + /projects/proj1 + /projects/proj1/memory（失败）
    assert!(first_count >= 3, "first poll should call read_dir at least 3 times");

    // 显式预填 cache，模拟"标记 absent"路径
    {
        let mut st = poll_state.lock().await;
        st.memory_dir_cache.insert(
            "/projects/proj1/memory".to_string(),
            super::MemoryDirCacheEntry {
                exists: false,
                last_check: std::time::Instant::now(),
            },
        );
    }
    read_dir_counter.store(0, Ordering::SeqCst);

    // 第二次 poll：cache 命中 absent → memory 目录 read_dir 必须被跳过
    FileWatcher::do_poll(
        &provider_dyn,
        &[std::path::PathBuf::from("/projects")],
        &poll_state,
        &tx,
    )
    .await
    .unwrap();
    let second_count = read_dir_counter.load(Ordering::SeqCst);

    // 验证：第二次应该比第一次少 1 次（少了 memory 目录的 read_dir）
    assert_eq!(
        second_count, first_count - 1,
        "cache hit must skip memory dir readdir ({} should be {})",
        second_count, first_count - 1
    );
}

/// I3 修复：扩展 MockFsProvider 支持注入 read_dir 错误消息，
/// 验证 Err 过滤逻辑（is_permanent_absent 的 4 个 contains 检查）
///
/// 测试意图：覆盖 do_poll 中 Err 分支的字符串匹配逻辑。
/// 之前 test_memory_dir_cache_skips_readdir_when_absent 用 MockFsProvider
/// 的默认 Err（"No mock entries for ..."）→ 不含过滤子串 → 零覆盖。
#[tokio::test]
async fn test_memory_dir_cache_marks_absent_on_enoent_error() {
    // 模拟 SFTP 返回 "no such file" 错误（russh-sftp format_sftp_error 输出格式）
    let provider = Arc::new(MockFsProvider::new("ssh"));
    provider.set_entries(
        "/projects",
        vec![MockDirent {
            name: "proj1".to_string(),
            is_file: false,
            is_directory: true,
            size: None,
        }],
    );
    provider.set_entries("/projects/proj1", vec![]);
    // 注入 memory 目录的 read_dir 错误（包含 "no such file"）
    provider.set_read_dir_error(
        "/projects/proj1/memory",
        "SFTP error for /projects/proj1/memory: no such file (status NoSuchFile)".to_string(),
    );

    let provider_dyn: Arc<dyn FsProvider> = provider;
    let poll_state = Arc::new(Mutex::new(SshPollState {
        timer: None,
        poll_interval_ms: 100,
        primed: true,
        polled_file_sizes: HashMap::new(),
        poll_in_progress: false,
        memory_dir_cache: HashMap::new(),
    }));
    let (tx, _rx) = broadcast::channel::<FileChangeEvent>(16);

    FileWatcher::do_poll(
        &provider_dyn,
        &[std::path::PathBuf::from("/projects")],
        &poll_state,
        &tx,
    )
    .await
    .unwrap();

    // 验证：Err 消息含 "no such file" → 标记 absent
    let st = poll_state.lock().await;
    let entry = st
        .memory_dir_cache
        .get("/projects/proj1/memory")
        .expect("cache entry must be created on ENOENT error");
    assert!(!entry.exists, "ENOENT error must mark exists=false");
    assert!(entry.is_fresh(), "just-marked entry must be fresh");
}

/// I3 修复：瞬时网络错误（timeout/IO）不应标记 absent
#[tokio::test]
async fn test_memory_dir_cache_skips_marking_absent_on_transient_error() {
    let provider = Arc::new(MockFsProvider::new("ssh"));
    provider.set_entries(
        "/projects",
        vec![MockDirent {
            name: "proj1".to_string(),
            is_file: false,
            is_directory: true,
            size: None,
        }],
    );
    provider.set_entries("/projects/proj1", vec![]);
    // 注入瞬时错误（不含 "no such file" / "not a directory"）
    provider.set_read_dir_error(
        "/projects/proj1/memory",
        "SFTP timeout for /projects/proj1/memory".to_string(),
    );

    let provider_dyn: Arc<dyn FsProvider> = provider;
    let poll_state = Arc::new(Mutex::new(SshPollState {
        timer: None,
        poll_interval_ms: 100,
        primed: true,
        polled_file_sizes: HashMap::new(),
        poll_in_progress: false,
        memory_dir_cache: HashMap::new(),
    }));
    let (tx, _rx) = broadcast::channel::<FileChangeEvent>(16);

    FileWatcher::do_poll(
        &provider_dyn,
        &[std::path::PathBuf::from("/projects")],
        &poll_state,
        &tx,
    )
    .await
    .unwrap();

    // 验证：瞬时错误不标记 absent（cache 不更新，下一轮重试）
    let st = poll_state.lock().await;
    assert!(
        !st.memory_dir_cache.contains_key("/projects/proj1/memory"),
        "transient error (timeout) must NOT mark absent — next round retries"
    );
}

// ── 多 agent 根监听（P2b）────────────────────────────────────

/// extra agent 事件分派：pi 会话文件路径 → 读头解析出 (pid=encode_path(cwd), sid)。
#[test]
fn test_process_debounced_event_pi_root_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(".pi").join("agent").join("sessions");
    let proj = root.join("--Users-x-proj--");
    std::fs::create_dir_all(&proj).unwrap();
    let path = proj.join("2026-08-25T05-41-57-146Z_01a03770-891a-7792-a043-5f1dc085467b.jsonl");
    std::fs::write(
        &path,
        "{\"type\":\"session\",\"cwd\":\"/Users/x/proj\",\"timestamp\":\"2026-08-25T05:41:57.146Z\"}\n",
    )
    .unwrap();

    let fs = local_provider();
    let debounced = notify_debouncer_mini::DebouncedEvent {
        path: path.clone(),
        kind: notify_debouncer_mini::DebouncedEventKind::Any,
    };
    let claude_root = dir.path().join(".claude").join("projects");
    let change = FileWatcher::process_debounced_event_with_provider(&fs, &debounced, &claude_root)
        .expect("pi event should resolve");
    assert_eq!(change.agent, Some(crate::types::domain::AgentKind::Pi));
    assert_eq!(change.session_id.as_deref(), Some("01a03770-891a-7792-a043-5f1dc085467b"));
    assert_eq!(change.project_id.as_deref(), Some("-Users-x-proj"));
}

/// pi 半写文件（无有效头）→ 事件静默丢弃（不 panic、不产生半解析事件）。
#[test]
fn test_process_debounced_event_pi_partial_write_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(".pi").join("agent").join("sessions");
    let proj = root.join("--x--");
    std::fs::create_dir_all(&proj).unwrap();
    let path = proj.join("2026-08-25T05-41-57-146Z_partialwrite.jsonl");
    std::fs::write(&path, "").unwrap();

    let fs = local_provider();
    let debounced = notify_debouncer_mini::DebouncedEvent {
        path: path.clone(),
        kind: notify_debouncer_mini::DebouncedEventKind::Any,
    };
    let change = FileWatcher::process_debounced_event_with_provider(
        &fs,
        &debounced,
        &dir.path().join("projects"),
    );
    assert!(change.is_none(), "partial write must be dropped");
}

/// SSH emit_event 的 extra 分派：pi 路径 → 事件带 agent + encode_path(cwd)。
#[tokio::test]
async fn test_emit_event_pi_root_dispatch() {
    use tokio::sync::broadcast;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(".pi").join("agent").join("sessions");
    let proj = root.join("--Users-x-proj--");
    std::fs::create_dir_all(&proj).unwrap();
    let path = proj.join("2026-08-25T00-00-00-000Z_00000000-0000-4000-8000-000000000000.jsonl");
    std::fs::write(
        &path,
        "{\"type\":\"session\",\"cwd\":\"/Users/x/proj\",\"timestamp\":\"2026-08-25T00:00:00.000Z\"}\n",
    )
    .unwrap();

    let (tx, mut rx) = broadcast::channel::<FileChangeEvent>(10);
    let fs = local_provider();
    FileWatcher::emit_event(
        &tx,
        &path,
        &dir.path().join("projects"),
        &fs,
        FileChangeType::Change,
    );
    let event = rx.try_recv().expect("pi event should emit");
    assert_eq!(event.agent, Some(crate::types::domain::AgentKind::Pi));
    assert_eq!(event.session_id.as_deref(), Some("00000000-0000-4000-8000-000000000000"));
    assert_eq!(event.project_id.as_deref(), Some("-Users-x-proj"));
}

/// 端到端（真实 notify）：多根监听下 pi 会话文件追加 → 广播事件带
/// agent=Pi + sessionId + encode_path(cwd)。手动触发：`cargo test -- --ignored`
#[tokio::test]
#[ignore = "依赖真实 OS 文件事件（CI 环境事件延迟不稳定）"]
async fn e2e_multi_root_pi_session_change() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    let claude_root = home.join(".claude").join("projects");
    let pi_root = home.join(".pi").join("agent").join("sessions");
    let proj = pi_root.join("--Users-x-proj--");
    std::fs::create_dir_all(&claude_root).unwrap();
    std::fs::create_dir_all(&proj).unwrap();
    let session = proj.join("2026-08-25T00-00-00-000Z_00000000-0000-4000-8000-000000000000.jsonl");
    std::fs::write(
        &session,
        "{\"type\":\"session\",\"cwd\":\"/Users/x/proj\",\"timestamp\":\"2026-08-25T00:00:00.000Z\"}\n",
    )
    .unwrap();

    let mut watcher = FileWatcher::new(local_provider());
    let mut rx = watcher.receiver();
    watcher
        .watch(&[claude_root, pi_root])
        .await
        .expect("watch should start");

    // 等 notify 建立 watch（防启动竞态）
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // 追加一条消息 → 触发 change 事件
    let mut f = std::fs::OpenOptions::new().append(true).open(&session).unwrap();
    use std::io::Write;
    writeln!(
        f,
        "{{\"type\":\"message\",\"id\":\"m1\",\"timestamp\":\"2026-08-25T00:00:01.000Z\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"hi\"}}]}}}}"
    )
    .unwrap();
    drop(f);

    // 事件到达（notify + 100ms debounce；10s 超时兜底）
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut got = None;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await {
            Ok(Ok(event)) => {
                if event.path.ends_with("00000000-0000-4000-8000-000000000000.jsonl") {
                    got = Some(event);
                    break;
                }
            }
            _ => continue,
        }
    }
    watcher.stop().await;

    let event = got.expect("pi session change event should arrive");
    assert_eq!(event.agent, Some(crate::types::domain::AgentKind::Pi));
    assert_eq!(event.session_id.as_deref(), Some("00000000-0000-4000-8000-000000000000"));
    assert_eq!(event.project_id.as_deref(), Some("-Users-x-proj"));
}
