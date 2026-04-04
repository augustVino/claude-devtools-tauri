//! Persistence operations for NotificationManager: load, save, prune.

use log::{info, warn, error};

use crate::types::config::StoredNotification;

use super::{NotificationManager, MAX_NOTIFICATIONS};

impl NotificationManager {
    // =========================================================================
    // 持久化
    // =========================================================================

    /// 从磁盘 JSON 文件加载通知。
    pub(crate) async fn load_notifications(&self) {
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
    pub(crate) async fn save_notifications(&self) {
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
    pub(crate) fn prune_notifications(&self) {
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
}
