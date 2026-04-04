//! CRUD operations for NotificationManager: add, get, mark read, delete, stats.

use std::collections::HashMap;

use log::error;

use crate::types::config::{
    DetectedError, GetNotificationsOptions, GetNotificationsResult, NotificationStats,
    StoredNotification,
};

use super::{NotificationManager, MAX_NOTIFICATIONS};

impl NotificationManager {
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
        let created_at = super::emission::now_millis();
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
        if super::filtering::NotificationManagerExt::should_notify(self, &stored.error).await {
            self.show_native_notification(&stored.error).await;
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
}
