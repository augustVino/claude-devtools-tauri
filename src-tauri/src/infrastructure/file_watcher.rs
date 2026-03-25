//! FileWatcher service - Watches for changes in Claude Code project files.
//!
//! Responsibilities:
//! - Watch directories for session file changes
//! - Debounce rapid file events (100ms)
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

use crate::types::domain::FileChangeEvent;

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
    pub async fn watch(&mut self, path: &Path) -> Result<(), String> {
        if !path.exists() {
            return Err(format!("Path does not exist: {}", path.display()));
        }

        let mut is_watching = self.is_watching.lock().await;
        if *is_watching {
            return Err("Already watching a directory".to_string());
        }

        let sender = self.sender.clone();
        let path_owned = path.to_path_buf();

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
            .watch(&path_owned, RecursiveMode::Recursive)
            .map_err(|e| format!("Failed to start watcher: {}", e))?;

        // Spawn task to process debounced events
        tokio::spawn(async move {
            while let Some(debounced_event) = rx.recv().await {
                if let Some(change_event) = Self::process_debounced_event(&debounced_event) {
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
    fn process_debounced_event(event: &DebouncedEvent) -> Option<FileChangeEvent> {
        // Filter to only .jsonl and .json files
        let extension = event.path.extension()?.to_str()?;

        if extension != "jsonl" && extension != "json" {
            return None;
        }

        // Check if file exists to determine if this is add or remove
        // (debouncer-mini doesn't distinguish - only Any/AnyContinuous)
        let event_type = if event.path.exists() {
            // File exists: could be add or change
            // We'll report "change" for simplicity
            "change"
        } else {
            // File doesn't exist: it was removed
            "unlink"
        };

        Some(FileChangeEvent {
            event_type: event_type.to_string(),
            path: event.path.to_string_lossy().to_string(),
            project_id: None,
            session_id: None,
            is_subagent: false,
        })
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
}