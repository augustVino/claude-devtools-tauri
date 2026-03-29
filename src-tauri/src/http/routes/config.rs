//! Config 路由处理器（占位）。

use axum::{Json, extract::State, http::StatusCode};

use crate::http::state::HttpState;

pub async fn get_config(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn update_config(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn add_ignore_regex(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn remove_ignore_regex(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn add_ignore_repository(
    State(_state): State<HttpState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn remove_ignore_repository(
    State(_state): State<HttpState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn snooze(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn clear_snooze(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn get_triggers(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn add_trigger(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn update_trigger(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn remove_trigger(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn test_trigger(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn pin_session(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn unpin_session(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn hide_session(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn unhide_session(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn hide_sessions(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn unhide_sessions(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

/// No-op handler for routes that require native UI interaction (select-folders, open-in-editor).
pub async fn no_op() -> (StatusCode, Json<super::SuccessResponse>) {
    super::success_json()
}
