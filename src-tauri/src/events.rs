//! Event emission helpers for Tauri.
//!
//! Provides typed event emission for file changes, todo changes, etc.

use tauri::{AppHandle, Emitter};

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