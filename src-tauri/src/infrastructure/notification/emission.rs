//! Native OS notification display and Tauri event emission for NotificationManager.

use log::{error, warn};
use tauri::Emitter;

use crate::types::config::{DetectedError, StoredNotification};

use super::NotificationManager;

impl NotificationManager {
    // =========================================================================
    // 原生通知与事件
    // =========================================================================

    /// 为错误显示原生操作系统通知。
    pub(crate) async fn show_native_notification(&self, error: &DetectedError) {
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

        let sound_enabled = self.config_manager.get_config().await.notifications.sound_enabled;
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
    pub(crate) fn emit_new_notification(&self, notification: &StoredNotification) {
        let Some(ref app_handle) = self.app_handle else {
            return;
        };

        if let Err(e) = app_handle.emit("notification:new", notification) {
            error!("Failed to emit notification:new event: {e}");
        }
    }

    /// 向前端发出 `notification:updated` 事件，携带总数/未读数。
    pub(crate) fn emit_notification_updated(&self) {
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
pub(crate) fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// 将字符串截断至 `max_len` 个字符。
pub(crate) fn truncate_str(s: &str, max_len: usize) -> String {
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
