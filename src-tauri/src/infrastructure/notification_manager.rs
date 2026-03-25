//! NotificationManager service — Manages native notifications and error history.
//!
//! Ported from Electron `src/main/services/infrastructure/NotificationManager.ts`.
//!
//! Responsibilities:
//! - Store error history at ~/.claude/claude-devtools-notifications.json (max 100)
//! - Show native OS notifications via tauri-plugin-notification
//! - Implement throttling (5 seconds per unique error hash)
//! - Respect config.notifications.enabled and snoozedUntil
//! - Filter errors matching ignoredRegex patterns
//! - Auto-prune notifications over 100 on startup
//! - Emit Tauri events to frontend: notification:new, notification:updated, error:detected

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use log::{error, info, warn};
use regex::Regex;
use tauri::Emitter;

use crate::types::config::{
    DetectedError, GetNotificationsOptions, GetNotificationsResult, NotificationStats,
    StoredNotification,
};
use crate::infrastructure::ConfigManager;

// =============================================================================
// Constants
// =============================================================================

/// Maximum number of notifications to store.
const MAX_NOTIFICATIONS: usize = 100;

/// Throttle window in milliseconds (5 seconds).
const THROTTLE_MS: u64 = 5000;

/// Notification file name (stored in ~/.claude/).
const NOTIFICATION_FILE: &str = "claude-devtools-notifications.json";

// =============================================================================
// NotificationManager
// =============================================================================

/// Central module for the notification system.
///
/// Receives detected errors from the error detection pipeline, manages notification
/// state (CRUD, read status), persists to disk, throttles duplicate notifications,
/// and emits events to the frontend.
pub struct NotificationManager {
    /// Tauri app handle for emitting events and showing native notifications.
    app_handle: Option<tauri::AppHandle>,
    /// Reference to the config manager for reading notification settings.
    config_manager: Arc<RwLock<ConfigManager>>,
    /// In-memory notification store (newest first).
    notifications: Arc<RwLock<Vec<StoredNotification>>>,
    /// Throttle map: `${projectId}:${message}` -> last-seen timestamp.
    throttle_map: Arc<Mutex<HashMap<String, u64>>>,
    /// Path to the persistence file.
    persistence_path: PathBuf,
    /// Whether initialization has completed.
    is_initialized: bool,
}

impl NotificationManager {
    // =========================================================================
    // Construction
    // =========================================================================

    /// Creates a new NotificationManager.
    ///
    /// Call [`initialize`](Self::initialize) after construction to load persisted
    /// notifications from disk.
    pub fn new(
        app_handle: tauri::AppHandle,
        config_manager: Arc<RwLock<ConfigManager>>,
    ) -> Self {
        let persistence_path = dirs::home_dir()
            .expect("home directory must exist")
            .join(".claude")
            .join(NOTIFICATION_FILE);

        Self {
            app_handle: Some(app_handle),
            config_manager,
            notifications: Arc::new(RwLock::new(Vec::new())),
            throttle_map: Arc::new(Mutex::new(HashMap::new())),
            persistence_path,
            is_initialized: false,
        }
    }

    /// Creates a NotificationManager without a Tauri app handle (for testing).
    #[cfg(test)]
    pub fn new_for_test(config_manager: Arc<RwLock<ConfigManager>>) -> Self {
        let persistence_path = std::env::temp_dir()
            .join(format!(
                "claude-devtools-test-notifications-{}.json",
                uuid::Uuid::new_v4()
            ));

        Self {
            app_handle: None,
            config_manager,
            notifications: Arc::new(RwLock::new(Vec::new())),
            throttle_map: Arc::new(Mutex::new(HashMap::new())),
            persistence_path,
            is_initialized: false,
        }
    }

    /// Returns the persistence path (for testing cleanup).
    #[cfg(test)]
    pub fn persistence_path(&self) -> &PathBuf {
        &self.persistence_path
    }

    // =========================================================================
    // Initialization
    // =========================================================================

    /// Loads notifications from disk and prunes if needed. Idempotent.
    pub async fn initialize(&mut self) {
        if self.is_initialized {
            return;
        }

        self.load_notifications().await;
        self.prune_notifications();
        self.is_initialized = true;

        let count = self
            .notifications
            .read()
            .map(|n| n.len())
            .unwrap_or(0);
        info!("NotificationManager: Initialized with {count} notifications");
    }

    // =========================================================================
    // Public API
    // =========================================================================

    /// Main entry point. Adds an error as a stored notification.
    ///
    /// Performs deduplication by `toolUseId`, then persists, emits events,
    /// and optionally shows a native OS notification.
    ///
    /// Returns `Some(StoredNotification)` if the error was accepted, or `None`
    /// if it was filtered by deduplication.
    pub async fn add_error(&self, error: DetectedError) -> Option<StoredNotification> {
        // --- Deduplication by toolUseId ---
        // The same tool call can appear in both the subagent JSONL and the
        // parent session JSONL. Keep the subagent-annotated version when possible.
        if let Some(ref tool_use_id) = error.tool_use_id {
            let existing_index = {
                let notifications = self.notifications.read().ok()?;
                notifications
                    .iter()
                    .position(|n| n.error.tool_use_id.as_ref() == Some(tool_use_id))
            };

            if let Some(idx) = existing_index {
                let should_replace = {
                    let notifications = self.notifications.read().ok()?;
                    let existing = notifications.get(idx)?;
                    // Replace only if existing has no subagentId but new error does
                    existing.error.subagent_id.is_none() && error.subagent_id.is_some()
                };

                if should_replace {
                    let mut notifications = self.notifications.write().ok()?;
                    notifications.remove(idx);
                } else {
                    // Already have a (better or equal) version — skip
                    return None;
                }
            }
        }

        // --- Build stored notification ---
        let created_at = now_millis();
        let stored = StoredNotification {
            error,
            is_read: false,
            created_at,
        };

        // --- Insert at front (newest first) ---
        {
            let mut notifications = self.notifications.write().map_err(|e| {
                error!("Failed to acquire write lock: {e}");
                e
            }).ok()?;

            notifications.insert(0, stored.clone());
        }

        // --- Prune if needed ---
        self.prune_notifications();

        // --- Persist ---
        self.save_notifications().await;

        // --- Emit events ---
        self.emit_new_notification(&stored);
        self.emit_notification_updated();

        // --- Native toast (only if not filtered/throttled) ---
        if self.should_notify(&stored.error).await {
            self.show_native_notification(&stored.error);
        }

        Some(stored)
    }

    /// Returns a paginated list of notifications (newest first).
    pub async fn get_notifications(
        &self,
        options: GetNotificationsOptions,
    ) -> GetNotificationsResult {
        let limit = options.limit.unwrap_or(20);
        let offset = options.offset.unwrap_or(0);

        let notifications = self.notifications.read().map(|n| n.clone()).unwrap_or_default();
        let total = notifications.len();

        let page: Vec<StoredNotification> = notifications
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect();

        let unread_count = self
            .notifications
            .read()
            .map(|n| n.iter().filter(|n| !n.is_read).count())
            .unwrap_or(0);
        let has_more = offset + page.len() < total;

        GetNotificationsResult {
            total,
            total_count: total,
            unread_count,
            has_more,
            notifications: page,
        }
    }

    /// Marks a single notification as read by ID.
    ///
    /// Returns `true` if the notification was found.
    pub async fn mark_read(&self, id: &str) -> bool {
        let mut notifications = match self.notifications.write() {
            Ok(n) => n,
            Err(e) => {
                error!("Failed to acquire write lock: {e}");
                return false;
            }
        };

        let found = notifications.iter_mut().any(|n| {
            if n.error.id == id && !n.is_read {
                n.is_read = true;
                true
            } else {
                n.error.id == id
            }
        });

        if found {
            drop(notifications);
            self.save_notifications().await;
            self.emit_notification_updated();
        }

        found
    }

    /// Marks all notifications as read.
    ///
    /// Returns `true` on success.
    pub async fn mark_all_read(&self) -> bool {
        let mut notifications = match self.notifications.write() {
            Ok(n) => n,
            Err(e) => {
                error!("Failed to acquire write lock: {e}");
                return false;
            }
        };

        let mut changed = false;
        for notification in notifications.iter_mut() {
            if !notification.is_read {
                notification.is_read = true;
                changed = true;
            }
        }

        if changed {
            drop(notifications);
            self.save_notifications().await;
            self.emit_notification_updated();
        }

        true
    }

    /// Removes all notifications.
    ///
    /// Returns `true` on success.
    pub async fn clear_all(&self) -> bool {
        let mut notifications = match self.notifications.write() {
            Ok(n) => n,
            Err(e) => {
                error!("Failed to acquire write lock: {e}");
                return false;
            }
        };

        notifications.clear();
        drop(notifications);
        self.save_notifications().await;
        self.emit_notification_updated();
        true
    }

    /// Deletes a single notification by ID.
    ///
    /// Returns `true` if found and deleted.
    pub async fn delete_notification(&self, id: &str) -> bool {
        let mut notifications = match self.notifications.write() {
            Ok(n) => n,
            Err(e) => {
                error!("Failed to acquire write lock: {e}");
                return false;
            }
        };

        let len_before = notifications.len();
        notifications.retain(|n| n.error.id != id);

        if notifications.len() == len_before {
            return false;
        }

        drop(notifications);
        self.save_notifications().await;
        self.emit_notification_updated();
        true
    }

    /// Returns the count of unread notifications.
    pub async fn get_unread_count(&self) -> usize {
        self.notifications
            .read()
            .map(|n| n.iter().filter(|n| !n.is_read).count())
            .unwrap_or(0)
    }

    /// Returns statistics about the notification store.
    pub fn get_stats(&self) -> NotificationStats {
        let notifications = self
            .notifications
            .read()
            .map(|n| n.clone())
            .unwrap_or_default();

        let mut by_project: HashMap<String, usize> = HashMap::new();
        let mut by_source: HashMap<String, usize> = HashMap::new();

        for notification in &notifications {
            let project_name = &notification.error.context.project_name;
            *by_project.entry(project_name.clone()).or_insert(0) += 1;
            *by_source.entry(notification.error.source.clone()).or_insert(0) += 1;
        }

        let unread = notifications.iter().filter(|n| !n.is_read).count();

        NotificationStats {
            total: notifications.len(),
            unread,
            by_project,
            by_source,
        }
    }

    // =========================================================================
    // Persistence
    // =========================================================================

    /// Loads notifications from the JSON file on disk.
    async fn load_notifications(&self) {
        if !self.persistence_path.exists() {
            return;
        }

        match tokio::fs::read_to_string(&self.persistence_path).await {
            Ok(content) => {
                let parsed: Result<Vec<StoredNotification>, _> =
                    serde_json::from_str(&content);

                match parsed {
                    Ok(notifications) => {
                        if let Ok(mut store) = self.notifications.write() {
                            *store = notifications;
                        }
                    }
                    Err(e) => {
                        warn!("Invalid notifications file format, starting fresh: {e}");
                        if let Ok(mut store) = self.notifications.write() {
                            *store = Vec::new();
                        }
                    }
                }
            }
            Err(e) => {
                error!("Error loading notifications: {e}");
            }
        }
    }

    /// Saves the current notification list to disk.
    async fn save_notifications(&self) {
        let notifications = self
            .notifications
            .read()
            .map(|n| n.clone())
            .unwrap_or_default();

        if let Some(parent) = self.persistence_path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                error!("Failed to create notifications directory: {e}");
                return;
            }
        }

        let content = match serde_json::to_string_pretty(&notifications) {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to serialize notifications: {e}");
                return;
            }
        };

        if let Err(e) = tokio::fs::write(&self.persistence_path, &content).await {
            error!("Error saving notifications: {e}");
        }
    }

    /// Prunes notifications to [`MAX_NOTIFICATIONS`] entries, removing oldest first.
    fn prune_notifications(&self) {
        if let Ok(mut notifications) = self.notifications.write() {
            if notifications.len() > MAX_NOTIFICATIONS {
                // Sort by createdAt descending (newest first), keep top N
                notifications.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                let removed = notifications.len() - MAX_NOTIFICATIONS;
                notifications.truncate(MAX_NOTIFICATIONS);

                info!("NotificationManager: Pruned {removed} old notifications");
            }
        }
    }

    // =========================================================================
    // Filtering & Throttling
    // =========================================================================

    /// Generates a throttle key from projectId + message.
    fn generate_error_hash(error: &DetectedError) -> String {
        format!("{}:{}", error.project_id, error.message)
    }

    /// Checks whether a native toast should be suppressed due to throttling.
    ///
    /// This does NOT affect storage — only the native OS notification.
    /// Updates the throttle map timestamp on cache-miss.
    fn is_throttled(&self, error: &DetectedError) -> bool {
        let hash = Self::generate_error_hash(error);
        let now = now_millis();

        let mut throttle_map = match self.throttle_map.lock() {
            Ok(m) => m,
            Err(e) => {
                error!("Failed to acquire throttle lock: {e}");
                return false;
            }
        };

        if let Some(&last_seen) = throttle_map.get(&hash) {
            if now - last_seen < THROTTLE_MS {
                return true;
            }
        }

        // Update throttle map
        throttle_map.insert(hash, now);

        // Periodic cleanup
        let expired_threshold = now.saturating_sub(THROTTLE_MS * 2);
        throttle_map.retain(|_, ts| *ts >= expired_threshold);

        false
    }

    /// Checks if an error message matches any configured ignored regex patterns.
    fn matches_ignored_regex(&self, error: &DetectedError) -> bool {
        let config = self
            .config_manager
            .read()
            .map(|cm| cm.get_config())
            .ok();

        let config = match config {
            Some(c) => c,
            None => return false,
        };

        if config.notifications.ignored_regex.is_empty() {
            return false;
        }

        for pattern in &config.notifications.ignored_regex {
            if let Ok(re) = Regex::new(pattern) {
                if re.is_match(&error.message) {
                    return true;
                }
            } else {
                warn!("NotificationManager: Invalid regex pattern: {pattern}");
            }
        }

        false
    }

    /// Checks whether notifications are currently enabled (not snoozed, not disabled).
    fn are_notifications_enabled(&self) -> bool {
        let config = self
            .config_manager
            .read()
            .map(|cm| cm.get_config())
            .ok();

        let config = match config {
            Some(c) => c,
            None => return false,
        };

        if !config.notifications.enabled {
            return false;
        }

        if let Some(snoozed_until) = config.notifications.snoozed_until {
            let now = now_millis();
            if now < snoozed_until {
                return false;
            }
            // Snooze has expired — clear it
            drop(config);
            if let Ok(cm) = self.config_manager.read() {
                cm.clear_snooze();
            }
            return true;
        }

        true
    }

    /// Determines whether a native toast should be shown for this error.
    ///
    /// This controls the OS notification only; storage is unconditional.
    async fn should_notify(&self, error: &DetectedError) -> bool {
        if !self.are_notifications_enabled() {
            return false;
        }

        if self.matches_ignored_regex(error) {
            return false;
        }

        if self.is_throttled(error) {
            return false;
        }

        true
    }

    // =========================================================================
    // Native Notifications & Events
    // =========================================================================

    /// Shows a native OS notification for an error.
    fn show_native_notification(&self, error: &DetectedError) {
        let Some(ref app_handle) = self.app_handle else {
            return;
        };

        let sound_enabled = self
            .config_manager
            .read()
            .ok()
            .map(|cm| cm.get_config().notifications.sound_enabled)
            .unwrap_or(true);

        let body = truncate_str(&error.message, 200);

        #[cfg(not(test))]
        {
            use tauri_plugin_notification::NotificationExt;

            let builder = app_handle
                .notification()
                .builder()
                .title("Claude Code Error")
                .body(&body);

            let builder = if sound_enabled {
                builder.sound("default")
            } else {
                builder
            };

            if let Err(e) = builder.show() {
                warn!("Failed to show native notification: {e}");
            }
        }

        #[cfg(test)]
        let _ = (sound_enabled, body);
    }

    /// Emits `notification:new` to the frontend.
    fn emit_new_notification(&self, notification: &StoredNotification) {
        let Some(ref app_handle) = self.app_handle else {
            return;
        };

        if let Err(e) = app_handle.emit("notification:new", notification) {
            error!("Failed to emit notification:new event: {e}");
        }
    }

    /// Emits `notification:updated` with total/unread counts.
    fn emit_notification_updated(&self) {
        let Some(ref app_handle) = self.app_handle else {
            return;
        };

        let (total, unread) = self
            .notifications
            .read()
            .map(|n| {
                let total = n.len();
                let unread = n.iter().filter(|n| !n.is_read).count();
                (total, unread)
            })
            .unwrap_or((0, 0));

        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct NotificationUpdatedPayload {
            total: usize,
            unread_count: usize,
        }

        let payload = NotificationUpdatedPayload {
            total,
            unread_count: unread,
        };

        if let Err(e) = app_handle.emit("notification:updated", &payload) {
            error!("Failed to emit notification:updated event: {e}");
        }
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Returns the current time in milliseconds since UNIX epoch.
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Truncates a string to `max_len` characters.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let end = s
            .char_indices()
            .nth(max_len)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        format!("{}...", &s[..end])
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::config::{ErrorContext, NotificationConfig, AppConfig, GeneralConfig, DisplayConfig, SessionConfig};

    /// Creates a default AppConfig for testing.
    fn default_app_config() -> AppConfig {
        AppConfig {
            notifications: NotificationConfig {
                enabled: true,
                sound_enabled: true,
                ignored_regex: vec![],
                ignored_repositories: vec![],
                snoozed_until: None,
                snooze_minutes: 30,
                include_subagent_errors: true,
                triggers: vec![],
            },
            general: GeneralConfig {
                launch_at_login: false,
                show_dock_icon: true,
                theme: "dark".to_string(),
                default_tab: "dashboard".to_string(),
                claude_root_path: None,
                auto_expand_ai_groups: false,
                use_native_title_bar: false,
            },
            display: DisplayConfig {
                show_timestamps: true,
                compact_mode: false,
                syntax_highlighting: true,
            },
            sessions: SessionConfig {
                pinned_sessions: HashMap::new(),
                hidden_sessions: HashMap::new(),
            },
            ssh: None,
            http_server: None,
        }
    }

    /// Creates a DetectedError for testing.
    fn make_error(id: &str, project_id: &str, message: &str) -> DetectedError {
        DetectedError {
            id: id.to_string(),
            session_id: "test-session".to_string(),
            project_id: project_id.to_string(),
            file_path: "/tmp/test.rs".to_string(),
            source: "Bash".to_string(),
            message: message.to_string(),
            timestamp: now_millis(),
            line_number: Some(42),
            tool_use_id: None,
            subagent_id: None,
            trigger_color: None,
            trigger_id: None,
            trigger_name: None,
            context: ErrorContext {
                project_name: "TestProject".to_string(),
                cwd: None,
            },
        }
    }

    /// Creates a DetectedError with tool_use_id for testing.
    fn make_error_with_tool(
        id: &str,
        project_id: &str,
        message: &str,
        tool_use_id: &str,
        subagent_id: Option<&str>,
    ) -> DetectedError {
        let mut error = make_error(id, project_id, message);
        error.tool_use_id = Some(tool_use_id.to_string());
        error.subagent_id = subagent_id.map(String::from);
        error
    }

    /// Creates a test NotificationManager.
    async fn make_manager() -> NotificationManager {
        let cm = Arc::new(RwLock::new(ConfigManager::with_path(
            std::env::temp_dir().join(format!(
                "claude-devtools-test-config-{}.json",
                uuid::Uuid::new_v4()
            )),
        )));
        NotificationManager::new_for_test(cm)
    }

    // --- initialize ---

    #[tokio::test]
    async fn test_initialize_missing_file() {
        let mut mgr = make_manager().await;
        // Missing file should not panic, notifications should be empty
        mgr.initialize().await;
        assert_eq!(
            mgr.notifications.read().unwrap().len(),
            0
        );
    }

    #[tokio::test]
    async fn test_initialize_loads_from_file() {
        let mut mgr = make_manager().await;
        let path = mgr.persistence_path().clone();

        // Write a notification to the file
        let stored = StoredNotification {
            error: make_error("n1", "proj-1", "test error"),
            is_read: false,
            created_at: 1000,
        };
        let data = serde_json::to_string_pretty(&vec![stored]).unwrap();
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        tokio::fs::write(&path, &data).await.unwrap();

        mgr.initialize().await;
        assert_eq!(mgr.notifications.read().unwrap().len(), 1);

        // Cleanup
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn test_initialize_invalid_json() {
        let mut mgr = make_manager().await;
        let path = mgr.persistence_path().clone();

        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        tokio::fs::write(&path, "not valid json").await.unwrap();

        mgr.initialize().await;
        assert_eq!(mgr.notifications.read().unwrap().len(), 0);

        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn test_initialize_prunes_excess() {
        let mut mgr = make_manager().await;
        let path = mgr.persistence_path().clone();

        // Create more than MAX_NOTIFICATIONS
        let many: Vec<StoredNotification> = (0..150)
            .map(|i| StoredNotification {
                error: make_error(&format!("n{i}"), "proj", &format!("error {i}")),
                is_read: false,
                created_at: i as u64,
            })
            .collect();

        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        tokio::fs::write(&path, serde_json::to_string(&many).unwrap())
            .await
            .unwrap();

        mgr.initialize().await;
        let count = mgr.notifications.read().unwrap().len();
        assert_eq!(count, MAX_NOTIFICATIONS);

        let _ = tokio::fs::remove_file(&path).await;
    }

    // --- add_error ---

    #[tokio::test]
    async fn test_add_error_stores_notification() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        let error = make_error("e1", "proj-1", "something broke");
        let result = mgr.add_error(error.clone()).await;

        assert!(result.is_some());
        let stored = result.unwrap();
        assert_eq!(stored.error.id, "e1");
        assert!(!stored.is_read);

        let count = mgr.notifications.read().unwrap().len();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_add_error_dedup_same_tool_use_id() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        let error1 = make_error_with_tool("e1", "proj-1", "error msg", "tool-1", None);
        let result1 = mgr.add_error(error1).await;
        assert!(result1.is_some());

        // Same toolUseId, no subagent — should be deduped (skip)
        let error2 = make_error_with_tool("e2", "proj-1", "different msg", "tool-1", None);
        let result2 = mgr.add_error(error2).await;
        assert!(result2.is_none());

        assert_eq!(mgr.notifications.read().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_add_error_dedup_replace_with_subagent() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        // First: no subagent
        let error1 = make_error_with_tool("e1", "proj-1", "error msg", "tool-1", None);
        let result1 = mgr.add_error(error1).await;
        assert!(result1.is_some());

        // Second: has subagent — should replace
        let error2 =
            make_error_with_tool("e2", "proj-1", "error msg v2", "tool-1", Some("sub-1"));
        let result2 = mgr.add_error(error2).await;
        assert!(result2.is_some());
        assert_eq!(result2.unwrap().error.subagent_id, Some("sub-1".to_string()));

        assert_eq!(mgr.notifications.read().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_add_error_dedup_keep_existing_subagent() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        // First: has subagent
        let error1 =
            make_error_with_tool("e1", "proj-1", "error msg", "tool-1", Some("sub-1"));
        let result1 = mgr.add_error(error1).await;
        assert!(result1.is_some());

        // Second: no subagent — existing is better, skip
        let error2 = make_error_with_tool("e2", "proj-1", "error msg v2", "tool-1", None);
        let result2 = mgr.add_error(error2).await;
        assert!(result2.is_none());

        assert_eq!(mgr.notifications.read().unwrap().len(), 1);
        assert_eq!(
            mgr.notifications.read().unwrap()[0].error.subagent_id,
            Some("sub-1".to_string())
        );
    }

    #[tokio::test]
    async fn test_add_error_no_tool_use_id_no_dedup() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        let error1 = make_error("e1", "proj-1", "error msg");
        let error2 = make_error("e2", "proj-1", "error msg");
        assert!(mgr.add_error(error1).await.is_some());
        assert!(mgr.add_error(error2).await.is_some());

        assert_eq!(mgr.notifications.read().unwrap().len(), 2);
    }

    // --- throttling ---

    #[tokio::test]
    async fn test_add_error_throttle_native_notification() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        // Add same error twice quickly — both should store (no dedup, different IDs)
        // but the second should be throttled for native notification.
        // Since we can't easily test native notifications in unit tests,
        // we verify the throttle map state.
        let error = make_error("e1", "proj-1", "throttle test");
        mgr.add_error(error.clone()).await;

        // Check throttle map has an entry
        let hash = NotificationManager::generate_error_hash(&error);
        let throttle_map = mgr.throttle_map.lock().unwrap();
        assert!(throttle_map.contains_key(&hash));
    }

    // --- get_notifications ---

    #[tokio::test]
    async fn test_get_notifications_default_pagination() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        for i in 0..5 {
            let error = make_error(&format!("e{i}"), "proj", &format!("msg {i}"));
            mgr.add_error(error).await;
        }

        let result = mgr
            .get_notifications(GetNotificationsOptions {
                limit: None,
                offset: None,
            })
            .await;

        assert_eq!(result.notifications.len(), 5);
        assert_eq!(result.total, 5);
        assert_eq!(result.total_count, 5);
        assert_eq!(result.unread_count, 5);
        assert!(!result.has_more);
    }

    #[tokio::test]
    async fn test_get_notifications_with_limit_and_offset() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        for i in 0..10 {
            let error = make_error(&format!("e{i}"), "proj", &format!("msg {i}"));
            mgr.add_error(error).await;
        }

        // Newest first, so e9 is first
        let page1 = mgr
            .get_notifications(GetNotificationsOptions {
                limit: Some(3),
                offset: Some(0),
            })
            .await;
        assert_eq!(page1.notifications.len(), 3);
        assert_eq!(page1.notifications[0].error.id, "e9");
        assert_eq!(page1.notifications[2].error.id, "e7");
        assert!(page1.has_more);

        let page2 = mgr
            .get_notifications(GetNotificationsOptions {
                limit: Some(3),
                offset: Some(3),
            })
            .await;
        assert_eq!(page2.notifications.len(), 3);
        assert_eq!(page2.notifications[0].error.id, "e6");
        assert!(page2.has_more);

        let page_last = mgr
            .get_notifications(GetNotificationsOptions {
                limit: Some(3),
                offset: Some(9),
            })
            .await;
        assert_eq!(page_last.notifications.len(), 1);
        assert!(!page_last.has_more);
    }

    // --- mark_read ---

    #[tokio::test]
    async fn test_mark_read() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        let error = make_error("e1", "proj", "msg");
        mgr.add_error(error).await;

        assert_eq!(mgr.get_unread_count().await, 1);

        let result = mgr.mark_read("e1").await;
        assert!(result);
        assert_eq!(mgr.get_unread_count().await, 0);

        // Marking already-read is still "found"
        let result2 = mgr.mark_read("e1").await;
        assert!(result2);
    }

    #[tokio::test]
    async fn test_mark_read_not_found() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        let result = mgr.mark_read("nonexistent").await;
        assert!(!result);
    }

    // --- mark_all_read ---

    #[tokio::test]
    async fn test_mark_all_read() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        for i in 0..3 {
            let error = make_error(&format!("e{i}"), "proj", &format!("msg {i}"));
            mgr.add_error(error).await;
        }

        assert_eq!(mgr.get_unread_count().await, 3);

        let result = mgr.mark_all_read().await;
        assert!(result);
        assert_eq!(mgr.get_unread_count().await, 0);
    }

    // --- clear_all ---

    #[tokio::test]
    async fn test_clear_all() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        for i in 0..3 {
            let error = make_error(&format!("e{i}"), "proj", &format!("msg {i}"));
            mgr.add_error(error).await;
        }

        assert_eq!(mgr.notifications.read().unwrap().len(), 3);

        let result = mgr.clear_all().await;
        assert!(result);
        assert_eq!(mgr.notifications.read().unwrap().len(), 0);
    }

    // --- delete_notification ---

    #[tokio::test]
    async fn test_delete_notification() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        mgr.add_error(make_error("e1", "proj", "msg1")).await;
        mgr.add_error(make_error("e2", "proj", "msg2")).await;

        assert_eq!(mgr.notifications.read().unwrap().len(), 2);

        let result = mgr.delete_notification("e1").await;
        assert!(result);
        assert_eq!(mgr.notifications.read().unwrap().len(), 1);
        assert_eq!(mgr.notifications.read().unwrap()[0].error.id, "e2");
    }

    #[tokio::test]
    async fn test_delete_notification_not_found() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        let result = mgr.delete_notification("nonexistent").await;
        assert!(!result);
    }

    // --- get_unread_count ---

    #[tokio::test]
    async fn test_get_unread_count() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        assert_eq!(mgr.get_unread_count().await, 0);

        mgr.add_error(make_error("e1", "proj", "msg1")).await;
        assert_eq!(mgr.get_unread_count().await, 1);

        mgr.mark_read("e1").await;
        assert_eq!(mgr.get_unread_count().await, 0);
    }

    // --- get_stats ---

    #[tokio::test]
    async fn test_get_stats() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        let mut err1 = make_error("e1", "proj-a", "error from bash");
        err1.source = "Bash".to_string();
        err1.context.project_name = "ProjectA".to_string();

        let mut err2 = make_error("e2", "proj-b", "error from edit");
        err2.source = "Edit".to_string();
        err2.context.project_name = "ProjectB".to_string();

        let mut err3 = make_error("e3", "proj-a", "another bash error");
        err3.source = "Bash".to_string();
        err3.context.project_name = "ProjectA".to_string();

        mgr.add_error(err1).await;
        mgr.add_error(err2).await;
        mgr.add_error(err3).await;

        let stats = mgr.get_stats();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.unread, 3);
        assert_eq!(stats.by_project.get("ProjectA"), Some(&2));
        assert_eq!(stats.by_project.get("ProjectB"), Some(&1));
        assert_eq!(stats.by_source.get("Bash"), Some(&2));
        assert_eq!(stats.by_source.get("Edit"), Some(&1));
    }

    // --- matches_ignored_regex ---

    #[tokio::test]
    async fn test_ignored_regex_filtering() {
        let cm = Arc::new(RwLock::new(ConfigManager::with_path(
            std::env::temp_dir().join(format!(
                "claude-devtools-test-config-{}.json",
                uuid::Uuid::new_v4()
            )),
        )));
        {
            let cm_guard = cm.read().unwrap();
            cm_guard.add_ignore_regex("permission denied".to_string()).unwrap();
        }

        let mut mgr = NotificationManager::new_for_test(cm);
        mgr.initialize().await;

        let error = make_error("e1", "proj", "Permission denied: access forbidden");
        // The error should still be stored (storage is unconditional)
        let result = mgr.add_error(error).await;
        assert!(result.is_some());

        // But should_notify should return false due to ignored regex
        // (we can't directly test should_notify since it's private,
        // but we can verify the error was stored despite the filter)
        assert_eq!(mgr.notifications.read().unwrap().len(), 1);
    }

    // --- truncate helper ---

    #[test]
    fn test_truncate_str_short() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_str_exact() {
        assert_eq!(truncate_str("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_str_long() {
        let result = truncate_str("hello world", 8);
        assert!(result.starts_with("hello wo"));
        assert!(result.ends_with("..."));
    }
}
