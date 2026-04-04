//! 文件监听器测试 — MockFsProvider + 全部单元/集成测试。

use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use super::*;
use crate::infrastructure::fs_provider::{FsDirent, FsStatResult, FsProvider, LocalFsProvider};

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

    // Phase 3: Back to local mode -- new watcher, clean state
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
