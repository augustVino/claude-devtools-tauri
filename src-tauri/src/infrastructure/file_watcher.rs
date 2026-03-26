//! FileWatcher service - Watches for changes in Claude Code project files.
//!
//! Responsibilities:
//! - Watch directories for session file changes
//! - Debounce rapid file events (100ms)
//! - Parse paths to extract projectId, sessionId, isSubagent
//! - Emit FileChangeEvent to subscribers

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::sync::Mutex;

// Import all notify types from notify_debouncer_mini to ensure version compatibility
// (notify-debouncer-mini uses notify v7 while direct dependency is v8)
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{new_debouncer, DebouncedEvent};

use crate::types::domain::{FileChangeEvent, FileChangeType};

/// Debounce interval in milliseconds (matches Electron implementation)
const DEBOUNCE_MS: u64 = 100;

/// Broadcast channel capacity for file change events
const CHANNEL_CAPACITY: usize = 64;

/// FileWatcher watches directories for file changes with debouncing.
pub struct FileWatcher {
    /// Inner debounced watcher (wrapped for async access)
    inner: Arc<Mutex<Option<DebouncedWatcher>>>,
    /// Broadcast sender for file change events
    sender: broadcast::Sender<FileChangeEvent>,
    /// Whether currently watching
    is_watching: Arc<Mutex<bool>>,
}

/// Wrapper for the debounced watcher handle
struct DebouncedWatcher {
    #[allow(dead_code)]
    watcher: notify_debouncer_mini::Debouncer<RecommendedWatcher>,
}

impl FileWatcher {
    /// Creates a new FileWatcher instance.
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(CHANNEL_CAPACITY);
        Self {
            inner: Arc::new(Mutex::new(None)),
            sender,
            is_watching: Arc::new(Mutex::new(false)),
        }
    }

    /// Starts watching a directory with debouncing.
    ///
    /// Only emits events for `.jsonl` and `.json` files.
    /// Parses paths to extract projectId, sessionId, and isSubagent.
    ///
    /// Path patterns:
    /// - Session file: `watchPath/projectId/sessionId.jsonl`
    /// - Subagent file: `watchPath/projectId/sessionId/subagents/agent-hash.jsonl`
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

        // Create debounced watcher with channel for events
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

        // Spawn task to process debounced events
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

    /// Stops watching the directory.
    pub async fn stop(&mut self) {
        let mut inner = self.inner.lock().await;
        let mut is_watching = self.is_watching.lock().await;

        if inner.take().is_some() {
            log::info!("FileWatcher: Stopped watching");
        }
        *is_watching = false;
    }

    /// Returns a receiver for file change events.
    pub fn receiver(&self) -> broadcast::Receiver<FileChangeEvent> {
        self.sender.subscribe()
    }

    /// Checks if currently watching a directory.
    pub async fn is_watching(&self) -> bool {
        *self.is_watching.lock().await
    }

    /// Processes a debounced event and converts to FileChangeEvent.
    ///
    /// Parses the file path relative to the watch path to extract:
    /// - projectId: First directory component after watch path
    /// - sessionId: Session ID from the path
    /// - isSubagent: Whether this is a subagent file
    ///
    /// Path patterns (mirrors Electron FileWatcher.ts):
    /// - Session file: `watchPath/projectId/sessionId.jsonl`
    /// - Subagent file: `watchPath/projectId/sessionId/subagents/agent-hash.jsonl`
    fn process_debounced_event(event: &DebouncedEvent, watch_path: &Path) -> Option<FileChangeEvent> {
        // Filter to only .jsonl and .json files
        let extension = event.path.extension()?.to_str()?;

        if extension != "jsonl" && extension != "json" {
            return None;
        }

        // Check if file exists to determine if this is add or remove
        // (debouncer-mini doesn't distinguish - only Any/AnyContinuous)
        let event_type = if event.path.exists() {
            // File exists: could be add or change
            // We'll report "change" for simplicity (Electron also reports "change" for most cases)
            FileChangeType::Change
        } else {
            // File doesn't exist: it was removed
            FileChangeType::Unlink
        };

        // Parse relative path to extract projectId, sessionId, isSubagent
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

    /// Parses path parts to extract projectId, sessionId, and isSubagent.
    ///
    /// Matches Electron FileWatcher.ts logic (lines 507-533):
    /// - Session file (2 parts): `projectId/sessionId.jsonl`
    /// - Subagent file (4 parts): `projectId/sessionId/subagents/agent-hash.jsonl`
    fn parse_path_parts(parts: &[&str]) -> (Option<String>, Option<String>, bool) {
        if parts.is_empty() {
            return (None, None, false);
        }

        let project_id = Some(parts[0].to_string());

        // Session file at project root: projectId/sessionId.jsonl
        if parts.len() == 2 && parts[1].ends_with(".jsonl") {
            let session_id = parts[1].strip_suffix(".jsonl").map(|s| s.to_string());
            return (project_id, session_id, false);
        }

        // Subagent file: projectId/sessionId/subagents/agent-hash.jsonl
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
        // Should not panic
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
        // Session file: projectId/sessionId.jsonl
        let parts = vec!["-Users-name-project", "session-abc123.jsonl"];
        let (project_id, session_id, is_subagent) = FileWatcher::parse_path_parts(&parts);

        assert_eq!(project_id, Some("-Users-name-project".to_string()));
        assert_eq!(session_id, Some("session-abc123".to_string()));
        assert!(!is_subagent);
    }

    #[test]
    fn test_parse_path_parts_subagent_file() {
        // Subagent file: projectId/sessionId/subagents/agent-hash.jsonl
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
        // Just a project directory, no session file
        let parts = vec!["-Users-name-project"];
        let (project_id, session_id, is_subagent) = FileWatcher::parse_path_parts(&parts);

        assert_eq!(project_id, Some("-Users-name-project".to_string()));
        assert_eq!(session_id, None);
        assert!(!is_subagent);
    }

    #[test]
    fn test_parse_path_parts_nested_directory() {
        // Nested directory that's not a subagent file
        let parts = vec!["-Users-name-project", "some-dir", "other.jsonl"];
        let (project_id, session_id, is_subagent) = FileWatcher::parse_path_parts(&parts);

        assert_eq!(project_id, Some("-Users-name-project".to_string()));
        assert_eq!(session_id, None);
        assert!(!is_subagent);
    }
}