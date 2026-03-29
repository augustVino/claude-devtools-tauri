//! Utility 路由处理器（占位）。

use axum::{Json, extract::State, http::StatusCode};

use crate::http::state::HttpState;

pub async fn get_version(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn read_claude_md(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn read_directory_claude_md(
    State(_state): State<HttpState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn read_mentioned_file(
    State(_state): State<HttpState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn read_agent_configs(
    State(_state): State<HttpState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

/// No-op handler for routes that require native UI interaction (open-path, open-external).
pub async fn no_op() -> (StatusCode, Json<super::SuccessResponse>) {
    super::success_json()
}
