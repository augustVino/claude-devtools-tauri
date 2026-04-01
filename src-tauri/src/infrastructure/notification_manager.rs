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
//! - Emit Tauri events to frontend: notification:new, notification:updated

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
use crate::parsing::git_identity::GitIdentityResolver;
use crate::utils::path_decoder;

// =============================================================================
// 常量
// =============================================================================

/// 通知存储最大数量。
const MAX_NOTIFICATIONS: usize = 100;

/// 节流窗口时长（毫秒，5 秒）。
const THROTTLE_MS: u64 = 5000;

/// 通知持久化文件名（存储于 ~/.claude/）。
const NOTIFICATION_FILE: &str = "claude-devtools-notifications.json";

// =============================================================================
// NotificationManager
// =============================================================================

/// 通知系统的核心模块。
///
/// 接收错误检测管道发现的错误，管理通知状态（增删改查、已读状态），
/// 持久化到磁盘，对重复通知进行节流，并向前端发出事件。
pub struct NotificationManager {
    /// Tauri 应用句柄，用于发出事件和显示原生通知。
    app_handle: Option<tauri::AppHandle>,
    /// 配置管理器引用，用于读取通知设置。
    config_manager: Arc<ConfigManager>,
    /// 内存中的通知存储（最新的在前）。
    notifications: Arc<RwLock<Vec<StoredNotification>>>,
    /// 节流映射表: `${projectId}:${message}` -> 上次出现的时间戳。
    throttle_map: Arc<Mutex<HashMap<String, u64>>>,
    /// 持久化文件路径。
    persistence_path: PathBuf,
    /// 初始化是否已完成。
    is_initialized: bool,
    /// 最后显示的原生通知对应的错误（独立于 RwLock 包装，供同步回调使用）。
    last_shown_error: Arc<Mutex<Option<DetectedError>>>,
    /// Regex 编译缓存：pattern → compiled regex (or None if invalid).
    regex_cache: Arc<Mutex<HashMap<String, Option<Regex>>>>,
}

impl NotificationManager {
    // =========================================================================
    // 构造
    // =========================================================================

    /// 创建新的 NotificationManager。
    ///
    /// 构造后需调用 [`initialize`](Self::initialize) 从磁盘加载已持久化的通知。
    pub fn new(
        app_handle: tauri::AppHandle,
        config_manager: Arc<ConfigManager>,
        last_shown_error: Arc<Mutex<Option<DetectedError>>>,
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
            last_shown_error,
            regex_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 创建无 Tauri 应用句柄的 NotificationManager（用于测试）。
    #[cfg(test)]
    pub fn new_for_test(config_manager: Arc<ConfigManager>) -> Self {
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
            last_shown_error: Arc::new(Mutex::new(None)),
            regex_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 返回持久化文件路径（用于测试清理）。
    #[cfg(test)]
    pub fn persistence_path(&self) -> &PathBuf {
        &self.persistence_path
    }

    // =========================================================================
    // 初始化
    // =========================================================================

    /// 从磁盘加载通知并在需要时进行裁剪。幂等操作。
    pub async fn initialize(&mut self) {
        if self.is_initialized {
            return;
        }

        self.load_notifications().await;
        self.prune_notifications();
        self.save_notifications().await;
        self.is_initialized = true;

        let count = self
            .notifications
            .read()
            .map(|n| n.len())
            .unwrap_or(0);
        info!("NotificationManager: Initialized with {count} notifications");
    }

    // =========================================================================
    // 公共 API
    // =========================================================================

    /// 主入口。将检测到的错误添加为已存储的通知。
    ///
    /// 按 `toolUseId` 进行去重，然后持久化、发出事件，
    /// 并可选地显示原生操作系统通知。
    ///
    /// 返回 `Some(StoredNotification)` 表示错误被接受，
    /// 或 `None` 表示被去重过滤掉。
    pub async fn add_error(&self, error: DetectedError) -> Option<StoredNotification> {
        // --- 按 toolUseId 去重 ---
        // 同一个工具调用可能同时出现在子代理 JSONL 和父会话 JSONL 中。
        // 优先保留带有子代理标注的版本。
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
                    // 仅当已有条目无 subagentId 而新错误有时才替换
                    existing.error.subagent_id.is_none() && error.subagent_id.is_some()
                };

                if should_replace {
                    let mut notifications = self.notifications.write().ok()?;
                    notifications.remove(idx);
                } else {
                    // 已有（更好或等同的）版本 — 跳过
                    return None;
                }
            }
        }

        // --- 构建已存储通知 ---
        let created_at = now_millis();
        let stored = StoredNotification {
            error,
            is_read: false,
            created_at,
        };

        // --- 插入到头部（最新的在前） ---
        {
            let mut notifications = self.notifications.write().map_err(|e| {
                error!("Failed to acquire write lock: {e}");
                e
            }).ok()?;

            notifications.insert(0, stored.clone());
        }

        // --- 需要时裁剪 ---
        self.prune_notifications();

        // --- 持久化 ---
        self.save_notifications().await;

        // --- 发出事件 ---
        self.emit_new_notification(&stored);
        self.emit_notification_updated();

        // --- 原生通知（仅当未被过滤/节流时） ---
        if self.should_notify(&stored.error).await {
            self.show_native_notification(&stored.error);
        }

        Some(stored)
    }

    /// 返回分页的通知列表（最新的在前）。
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

    /// 按 ID 将单条通知标记为已读。
    ///
    /// 如果找到通知则返回 `true`。
    pub async fn mark_read(&self, id: &str) -> bool {
        let found = {
            let mut notifications = match self.notifications.write() {
                Ok(n) => n,
                Err(e) => {
                    error!("Failed to acquire write lock: {e}");
                    return false;
                }
            };

            notifications.iter_mut().any(|n| {
                if n.error.id == id && !n.is_read {
                    n.is_read = true;
                    true
                } else {
                    n.error.id == id
                }
            })
        };
        // 写锁守卫在此处释放（块作用域结束）

        if found {
            self.save_notifications().await;
            self.emit_notification_updated();
        }

        found
    }

    /// 将所有通知标记为已读。
    ///
    /// 成功时返回 `true`。
    pub async fn mark_all_read(&self) -> bool {
        let changed = {
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
            changed
        };
        // 写锁守卫在此处释放（块作用域结束）

        if changed {
            self.save_notifications().await;
            self.emit_notification_updated();
        }

        true
    }

    /// 移除所有通知。
    ///
    /// 成功时返回 `true`。
    pub async fn clear_all(&self) -> bool {
        {
            let mut notifications = match self.notifications.write() {
                Ok(n) => n,
                Err(e) => {
                    error!("Failed to acquire write lock: {e}");
                    return false;
                }
            };

            notifications.clear();
        }
        // 写锁守卫在此处释放（块作用域结束）

        self.save_notifications().await;
        self.emit_notification_updated();
        true
    }

    /// 按 ID 删除单条通知。
    ///
    /// 如果找到并删除则返回 `true`。
    pub async fn delete_notification(&self, id: &str) -> bool {
        let found = {
            let mut notifications = match self.notifications.write() {
                Ok(n) => n,
                Err(e) => {
                    error!("Failed to acquire write lock: {e}");
                    return false;
                }
            };

            let len_before = notifications.len();
            notifications.retain(|n| n.error.id != id);
            notifications.len() != len_before
        };
        // 写锁守卫在此处释放（块作用域结束）

        if found {
            self.save_notifications().await;
            self.emit_notification_updated();
            true
        } else {
            false
        }
    }

    /// 返回未读通知数量。
    pub async fn get_unread_count(&self) -> usize {
        self.notifications
            .read()
            .map(|n| n.iter().filter(|n| !n.is_read).count())
            .unwrap_or(0)
    }

    /// 返回通知存储的统计信息。
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
    // 持久化
    // =========================================================================

    /// 从磁盘 JSON 文件加载通知。
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

    /// 将当前通知列表保存到磁盘。
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

    /// 将通知裁剪至 [`MAX_NOTIFICATIONS`] 条，优先移除最旧的。
    fn prune_notifications(&self) {
        if let Ok(mut notifications) = self.notifications.write() {
            if notifications.len() > MAX_NOTIFICATIONS {
                // 按 createdAt 降序排列（最新的在前），保留前 N 条
                notifications.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                let removed = notifications.len() - MAX_NOTIFICATIONS;
                notifications.truncate(MAX_NOTIFICATIONS);

                info!("NotificationManager: Pruned {removed} old notifications");
            }
        }
    }

    // =========================================================================
    // 过滤与节流
    // =========================================================================

    /// 从 projectId + message 生成节流键。
    fn generate_error_hash(error: &DetectedError) -> String {
        format!("{}:{}", error.project_id, error.message)
    }

    /// 检查是否因节流而抑制原生通知。
    ///
    /// 此检查不影响存储 — 仅影响原生操作系统通知。
    /// 缓存未命中时更新节流映射的时间戳。
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

        // 更新节流映射
        throttle_map.insert(hash, now);

        // 定期清理过期条目
        let expired_threshold = now.saturating_sub(THROTTLE_MS * 2);
        throttle_map.retain(|_, ts| *ts >= expired_threshold);

        false
    }

    /// 检查错误消息是否匹配任何已配置的忽略正则模式。
    fn matches_ignored_regex(&self, error: &DetectedError) -> bool {
        let config = self.config_manager.get_config();

        if config.notifications.ignored_regex.is_empty() {
            return false;
        }

        for pattern in &config.notifications.ignored_regex {
            let case_insensitive = format!("(?i){}", pattern);

            // 查缓存
            {
                if let Ok(cache) = self.regex_cache.lock() {
                    if let Some(cached) = cache.get(&case_insensitive) {
                        if let Some(ref re) = cached {
                            if re.is_match(&error.message) {
                                return true;
                            }
                        }
                        continue; // cached as None (invalid regex)
                    }
                }
            }

            // 未命中缓存，编译并缓存
            let compiled = crate::utils::regex_validation::create_safe_regex(&case_insensitive);
            let is_match = compiled.as_ref().map_or(false, |re| re.is_match(&error.message));

            if let Ok(mut cache) = self.regex_cache.lock() {
                if cache.len() >= 500 {
                    cache.clear();
                }
                cache.insert(case_insensitive, compiled);
            }

            if is_match {
                return true;
            }
        }

        false
    }

    /// Check if the error comes from an ignored repository.
    ///
    /// Resolves the error's projectId to a repository identity via GitIdentityResolver,
    /// then checks against config.notifications.ignored_repositories.
    fn is_from_ignored_repository(&self, error: &DetectedError) -> bool {
        let config = self.config_manager.get_config();
        let ignored = &config.notifications.ignored_repositories;
        if ignored.is_empty() {
            return false;
        }

        let resolver = GitIdentityResolver;
        let project_path = path_decoder::decode_path(&error.project_id);
        let resolved_path = error.context.cwd.as_deref().unwrap_or(&project_path);

        match resolver.resolve_identity(resolved_path) {
            Some(identity) => ignored.contains(&identity.id),
            None => false,
        }
    }

    /// 检查通知当前是否启用（未暂停、未禁用）。
    fn are_notifications_enabled(&self) -> bool {
        let config = self.config_manager.get_config();

        if !config.notifications.enabled {
            return false;
        }

        if let Some(snoozed_until) = config.notifications.snoozed_until {
            let now = now_millis();
            if now < snoozed_until {
                return false;
            }
            // 暂停已过期 — 清除暂停状态
            self.config_manager.clear_snooze();
            return true;
        }

        true
    }

    /// 判断是否应为该错误显示原生通知。
    ///
    /// 此方法仅控制操作系统通知；存储不受条件限制。
    async fn should_notify(&self, error: &DetectedError) -> bool {
        if !self.are_notifications_enabled() {
            return false;
        }

        if self.is_from_ignored_repository(error) {
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
    // 原生通知与事件
    // =========================================================================

    /// 为错误显示原生操作系统通知。
    fn show_native_notification(&self, error: &DetectedError) {
        let Some(ref app_handle) = self.app_handle else {
            return;
        };

        // Store error for click detection (independent Arc<Mutex<>> for sync access)
        {
            let mut last = match self.last_shown_error.lock() {
                Ok(guard) => guard,
                Err(e) => {
                    error!("Failed to acquire last_shown_error lock: {e}");
                    return;
                }
            };
            *last = Some(error.clone());
        }

        // Clear last_shown_error after 30 seconds to avoid stale click handling
        {
            let clear_handle = self.last_shown_error.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                if let Ok(mut last) = clear_handle.lock() {
                    *last = None;
                }
            });
        }

        let sound_enabled = self.config_manager.get_config().notifications.sound_enabled;
        let body = truncate_str(&error.message, 200);
        let subtitle = &error.context.project_name;

        #[cfg(not(test))]
        {
            // Use notify-rust directly for subtitle support.
            // tauri-plugin-notification v2 does not expose subtitle().
            let mut notification = notify_rust::Notification::new();
            notification
                .summary("Claude Code Error")
                .subtitle(subtitle)
                .body(&body);

            if sound_enabled {
                notification.sound_name("default");
            }

            #[cfg(target_os = "macos")]
            {
                let _ = notify_rust::set_application(
                    if tauri::is_dev() {
                        "com.apple.Terminal"
                    } else {
                        app_handle.config().identifier.as_str()
                    },
                );
            }

            if let Err(e) = notification.show() {
                warn!("Failed to show native notification: {e}");
            }
        }

        #[cfg(test)]
        let _ = (sound_enabled, body, subtitle);
    }

    /// 向前端发出 `notification:new` 事件。
    fn emit_new_notification(&self, notification: &StoredNotification) {
        let Some(ref app_handle) = self.app_handle else {
            return;
        };

        if let Err(e) = app_handle.emit("notification:new", notification) {
            error!("Failed to emit notification:new event: {e}");
        }
    }

    /// 向前端发出 `notification:updated` 事件，携带总数/未读数。
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
// 辅助函数
// =============================================================================

/// 返回自 UNIX 纪元以来的当前时间（毫秒）。
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// 将字符串截断至 `max_len` 个字符。
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
// 测试
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::config::{ErrorContext, NotificationConfig, AppConfig, GeneralConfig, DisplayConfig, SessionConfig};

    /// 创建用于测试的默认 AppConfig。
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

    /// 创建用于测试的 DetectedError。
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

    /// 创建带 tool_use_id 的 DetectedError（用于测试）。
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

    /// 创建用于测试的 NotificationManager。
    async fn make_manager() -> NotificationManager {
        let cm = Arc::new(ConfigManager::with_path(
            std::env::temp_dir().join(format!(
                "claude-devtools-test-config-{}.json",
                uuid::Uuid::new_v4()
            )),
        ));
        NotificationManager::new_for_test(cm)
    }

    // --- initialize ---

    #[tokio::test]
    async fn test_initialize_missing_file() {
        let mut mgr = make_manager().await;
        // 缺失文件不应 panic，通知列表应为空
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

        // 向文件写入一条通知
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

        // 清理
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

        // 创建超过 MAX_NOTIFICATIONS 的条目
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

        // 相同 toolUseId，无子代理 — 应被去重（跳过）
        let error2 = make_error_with_tool("e2", "proj-1", "different msg", "tool-1", None);
        let result2 = mgr.add_error(error2).await;
        assert!(result2.is_none());

        assert_eq!(mgr.notifications.read().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_add_error_dedup_replace_with_subagent() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        // 第一个: 无子代理
        let error1 = make_error_with_tool("e1", "proj-1", "error msg", "tool-1", None);
        let result1 = mgr.add_error(error1).await;
        assert!(result1.is_some());

        // 第二个: 有子代理 — 应替换
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

        // 第一个: 有子代理
        let error1 =
            make_error_with_tool("e1", "proj-1", "error msg", "tool-1", Some("sub-1"));
        let result1 = mgr.add_error(error1).await;
        assert!(result1.is_some());

        // 第二个: 无子代理 — 已有条目更优，跳过
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

        // 快速添加两次相同错误 — 两者都应存储（无去重，ID 不同）
        // 但第二次的原生通知应被节流。
        // 由于无法在单元测试中直接测试原生通知，
        // 因此验证节流映射的状态。
        let error = make_error("e1", "proj-1", "throttle test");
        mgr.add_error(error.clone()).await;

        // 检查节流映射中是否有条目
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

        // 最新的在前，所以 e9 排第一
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

        // 标记已读的条目仍视为"找到"
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
        let cm = Arc::new(ConfigManager::with_path(
            std::env::temp_dir().join(format!(
                "claude-devtools-test-config-{}.json",
                uuid::Uuid::new_v4()
            )),
        ));
        cm.add_ignore_regex("permission denied".to_string()).unwrap();

        let mut mgr = NotificationManager::new_for_test(cm);
        mgr.initialize().await;

        let error = make_error("e1", "proj", "Permission denied: access forbidden");
        // 错误仍应被存储（存储不受条件限制）
        let result = mgr.add_error(error).await;
        assert!(result.is_some());

        // 但 should_notify 应因忽略正则返回 false
        //（无法直接测试 should_notify 因为它是私有方法，
        // 但可以验证错误虽被过滤但仍已存储）
        assert_eq!(mgr.notifications.read().unwrap().len(), 1);
    }

    // --- 截断辅助函数 ---

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
