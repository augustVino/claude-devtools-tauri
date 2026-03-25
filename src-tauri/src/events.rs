//! Event emission helpers for Tauri.
//!
//! Provides typed event emission for file changes, todo changes,
//! notifications, and error detection events.

use tauri::{AppHandle, Emitter};

use crate::types::config::{DetectedError, StoredNotification};
use crate::types::domain::FileChangeEvent;

/// Emit a file change event to the frontend.
pub fn emit_file_change(app: &AppHandle, event: FileChangeEvent) {
    if let Err(e) = app.emit("file-change", &event) {
        log::error!("Failed to emit file-change event: {}", e);
    }
}

/// Todo change event payload.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoChangeEvent {
    pub session_id: String,
}

/// Emit a todo change event to the frontend.
pub fn emit_todo_change(app: &AppHandle, event: TodoChangeEvent) {
    if let Err(e) = app.emit("todo-change", &event) {
        log::error!("Failed to emit todo-change event: {}", e);
    }
}

/// Emit a new notification event to the frontend.
pub fn emit_notification_new(app: &AppHandle, notification: &StoredNotification) {
    if let Err(e) = app.emit("notification:new", notification) {
        log::error!("Failed to emit notification:new event: {}", e);
    }
}

/// Notification updated event payload.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationUpdatedPayload {
    pub total: usize,
    pub unread_count: usize,
}

/// Emit a notification state change to the frontend.
pub fn emit_notification_updated(
    app: &AppHandle,
    total: usize,
    unread_count: usize,
) {
    let payload = NotificationUpdatedPayload { total, unread_count };
    if let Err(e) = app.emit("notification:updated", &payload) {
        log::error!("Failed to emit notification:updated event: {}", e);
    }
}

/// Emit an error detected event.
pub fn emit_error_detected(app: &AppHandle, error: &DetectedError) {
    if let Err(e) = app.emit("error:detected", error) {
        log::error!("Failed to emit error:detected event: {}", e);
    }
}