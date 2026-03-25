use tauri::{command, State};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::infrastructure::NotificationManager;
use crate::types::config::{
    GetNotificationsOptions, GetNotificationsResult, NotificationCountResult, NotificationStats,
};

// =============================================================================
// Notification Commands
// =============================================================================

/// Returns a paginated list of notifications (newest first).
#[command]
pub async fn get_notifications(
    options: Option<GetNotificationsOptions>,
    notification_manager: State<'_, Arc<RwLock<NotificationManager>>>,
) -> Result<GetNotificationsResult, String> {
    let mgr = notification_manager.read().await;
    let opts = options.unwrap_or(GetNotificationsOptions {
        limit: None,
        offset: None,
    });
    Ok(mgr.get_notifications(opts).await)
}

/// Marks a single notification as read by ID.
///
/// Returns `true` if the notification was found.
#[command]
pub async fn mark_notification_read(
    notification_id: String,
    notification_manager: State<'_, Arc<RwLock<NotificationManager>>>,
) -> Result<bool, String> {
    let mgr = notification_manager.read().await;
    Ok(mgr.mark_read(&notification_id).await)
}

/// Marks all notifications as read.
///
/// Returns `true` on success.
#[command]
pub async fn mark_all_notifications_read(
    notification_manager: State<'_, Arc<RwLock<NotificationManager>>>,
) -> Result<bool, String> {
    let mgr = notification_manager.read().await;
    Ok(mgr.mark_all_read().await)
}

/// Deletes a single notification by ID.
///
/// Returns `true` if found and deleted.
#[command]
pub async fn delete_notification(
    notification_id: String,
    notification_manager: State<'_, Arc<RwLock<NotificationManager>>>,
) -> Result<bool, String> {
    let mgr = notification_manager.read().await;
    Ok(mgr.delete_notification(&notification_id).await)
}

/// Removes all notifications.
///
/// Returns `true` on success.
#[command]
pub async fn clear_notifications(
    notification_manager: State<'_, Arc<RwLock<NotificationManager>>>,
) -> Result<bool, String> {
    let mgr = notification_manager.read().await;
    Ok(mgr.clear_all().await)
}

/// Returns the total and unread notification counts.
#[command]
pub async fn get_notification_count(
    notification_manager: State<'_, Arc<RwLock<NotificationManager>>>,
) -> Result<NotificationCountResult, String> {
    let mgr = notification_manager.read().await;
    let unread_count = mgr.get_unread_count().await;
    let stats = mgr.get_stats();
    Ok(NotificationCountResult {
        total: stats.total,
        unread_count,
    })
}

/// Returns statistics about the notification store.
#[command]
pub async fn get_notification_stats(
    notification_manager: State<'_, Arc<RwLock<NotificationManager>>>,
) -> Result<NotificationStats, String> {
    let mgr = notification_manager.read().await;
    Ok(mgr.get_stats())
}
