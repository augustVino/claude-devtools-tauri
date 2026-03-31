use tauri::{command, Emitter, Manager, State};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::infrastructure::NotificationManager;
use crate::types::config::{
    DetectedError, GetNotificationsOptions, GetNotificationsResult, NotificationCountResult,
    NotificationStats,
};

// =============================================================================
// 验证辅助函数
// =============================================================================

/// Validate that a notification ID matches the expected format.
fn validate_notification_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 128 {
        return Err("Invalid notification ID format".to_string());
    }
    let first = id.chars().next();
    if !first.is_some_and(|c| c.is_alphanumeric()) {
        return Err("Invalid notification ID format".to_string());
    }
    if !id.chars().all(|c| c.is_alphanumeric() || c == '.' || c == '_' || c == '-') {
        return Err("Invalid notification ID format".to_string());
    }
    Ok(())
}

/// Clamp page limit to a reasonable range [1, 200].
fn coerce_page_limit(limit: Option<usize>, default: usize) -> usize {
    limit.unwrap_or(default).clamp(1, 200)
}

// =============================================================================
// 通知命令
// =============================================================================

/// 获取分页通知列表（按时间降序）。
#[command]
pub async fn get_notifications(
    options: Option<GetNotificationsOptions>,
    notification_manager: State<'_, Arc<RwLock<NotificationManager>>>,
) -> Result<GetNotificationsResult, String> {
    let mgr = notification_manager.read().await;
    let limit = coerce_page_limit(options.as_ref().and_then(|o| o.limit), 20);
    let offset = options.as_ref().and_then(|o| o.offset).unwrap_or(0);
    let opts = GetNotificationsOptions {
        limit: Some(limit),
        offset: Some(offset),
    };
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
    validate_notification_id(&notification_id)?;
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
    validate_notification_id(&notification_id)?;
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

/// Handle a notification click event from the frontend.
///
/// When the user clicks a native OS notification, the window gains focus.
/// The frontend detects this via a `focus` event listener and calls this command.
/// If a `last_shown_error` is present (set within the last 30 seconds),
/// this command focuses the window, emits `notification:clicked` with the error,
/// and clears the stored error.
#[command]
pub async fn handle_notification_click(
    app_handle: tauri::AppHandle,
    last_shown_error: State<'_, Arc<std::sync::Mutex<Option<DetectedError>>>>,
) -> Result<bool, String> {
    let error = {
        let mut guard = last_shown_error
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        guard.take()
    };

    let Some(error) = error else {
        return Ok(false);
    };

    // Focus the window
    if let Some(window) = app_handle.get_webview_window("main") {
        window
            .set_focus()
            .map_err(|e| format!("Failed to focus window: {e}"))?;
        #[cfg(target_os = "macos")]
        {
            use cocoa::appkit::NSApplication;
            use cocoa::base::nil;
            unsafe {
                let ns_app = NSApplication::sharedApplication(nil);
                NSApplication::activateIgnoringOtherApps_(ns_app, true);
            }
        }
    }

    // Emit notification:clicked event for the frontend
    app_handle
        .emit("notification:clicked", &error)
        .map_err(|e| format!("Failed to emit notification:clicked event: {e}"))?;

    // Bridge to SSE broadcaster for HTTP-only clients
    let broadcaster = app_handle.state::<crate::http::sse::SSEBroadcaster>().inner().clone();
    broadcaster.send(crate::http::sse::BackendEvent::NotificationClicked(error));

    Ok(true)
}

#[cfg(test)]
mod validation_tests {
    use super::*;

    #[test]
    fn valid_notification_id_passes() {
        assert!(validate_notification_id("abc123").is_ok());
        assert!(validate_notification_id("a.b_c-d").is_ok());
        assert!(validate_notification_id("550e8400-e29b-41d4-a716-446655440000").is_ok());
    }

    #[test]
    fn invalid_notification_id_rejected() {
        assert!(validate_notification_id("").is_err());
        assert!(validate_notification_id(".starts-with-dot").is_err());
        assert!(validate_notification_id("has spaces").is_err());
        assert!(validate_notification_id(&"a".repeat(129)).is_err());
    }

    #[test]
    fn coerce_page_limit_clamps() {
        assert_eq!(coerce_page_limit(None, 20), 20);
        assert_eq!(coerce_page_limit(Some(50), 20), 50);
        assert_eq!(coerce_page_limit(Some(999), 20), 200);
        assert_eq!(coerce_page_limit(Some(0), 20), 1);
    }
}
