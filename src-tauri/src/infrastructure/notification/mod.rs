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

mod crud;
mod emission;
mod filtering;
mod persistence;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use crate::types::config::DetectedError;
use crate::infrastructure::ConfigManager;

// =============================================================================
// 常量
// =============================================================================

/// 通知存储最大数量。
pub(crate) const MAX_NOTIFICATIONS: usize = 100;

/// 节流窗口时长（毫秒，5 秒）。
pub(crate) const THROTTLE_MS: u64 = 5000;

/// 通知持久化文件名（存储于 ~/.claude/）。
const NOTIFICATION_FILE: &str = "claude-devtools-notifications.json";

/// Regex 编译缓存的最大条目数。
pub(crate) const MAX_REGEX_CACHE_SIZE: usize = 500;

// =============================================================================
// NotificationManager
// =============================================================================

/// 通知系统的核心模块。
///
/// 接收错误检测管道发现的错误，管理通知状态（增删改查、已读状态），
/// 持久化到磁盘，对重复通知进行节流，并向前端发出事件。
pub struct NotificationManager {
    /// Tauri 应用句柄，用于发出事件和显示原生通知。
    pub(crate) app_handle: Option<tauri::AppHandle>,
    /// 配置管理器引用，用于读取通知设置。
    pub(crate) config_manager: Arc<ConfigManager>,
    /// 内存中的通知存储（最新的在前）。
    pub(crate) notifications: Arc<RwLock<Vec<crate::types::config::StoredNotification>>>,
    /// 节流映射表: `${projectId}:${message}` -> 上次出现的时间戳。
    pub(crate) throttle_map: Arc<Mutex<HashMap<String, u64>>>,
    /// 持久化文件路径。
    pub(crate) persistence_path: PathBuf,
    /// 初始化是否已完成。
    pub(crate) is_initialized: bool,
    /// 最后显示的原生通知对应的错误（独立于 RwLock 包装，供同步回调使用）。
    pub(crate) last_shown_error: Arc<Mutex<Option<DetectedError>>>,
    /// Regex 编译缓存：pattern -> compiled regex (or None if invalid).
    pub(crate) regex_cache: Arc<Mutex<HashMap<String, Option<regex::Regex>>>>,
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
        log::info!("NotificationManager: Initialized with {count} notifications");
    }
}
