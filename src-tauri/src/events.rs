//! Tauri 事件通知工具模块。
//!
//! 提供面向前端的事件通知函数，覆盖文件变更、待办事项变更、
//! 通知管理和错误检测等事件类型。

use tauri::{AppHandle, Emitter};

use crate::types::config::StoredNotification;
use crate::types::domain::FileChangeEvent;
use crate::infrastructure::context_manager::ContextInfo;

/// 通知前端文件变更事件。
pub fn emit_file_change(app: &AppHandle, event: FileChangeEvent) {
    if let Err(e) = app.emit("file-change", &event) {
        log::error!("Failed to emit file-change event: {}", e);
    }
}

/// 待办事项变更事件载荷。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoChangeEvent {
    pub session_id: String,
}

/// 通知前端待办事项变更事件。
pub fn emit_todo_change(app: &AppHandle, event: TodoChangeEvent) {
    if let Err(e) = app.emit("todo-change", &event) {
        log::error!("Failed to emit todo-change event: {}", e);
    }
}

/// 通知前端新通知事件。
#[allow(dead_code)]
pub fn emit_notification_new(app: &AppHandle, notification: &StoredNotification) {
    if let Err(e) = app.emit("notification:new", notification) {
        log::error!("Failed to emit notification:new event: {}", e);
    }
}

/// 通知状态更新事件载荷。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationUpdatedPayload {
    pub total: usize,
    pub unread_count: usize,
}

/// 通知前端通知状态变更事件（包含总数和未读数）。
#[allow(dead_code)]
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

/// 通知前端上下文已切换。
pub fn emit_context_changed(app: &AppHandle, info: &ContextInfo) {
    if let Err(e) = app.emit("context:changed", info) {
        log::error!("Failed to emit context:changed event: {}", e);
    }
}