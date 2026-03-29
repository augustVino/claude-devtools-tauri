//! Validation 路由处理器（占位）。

use axum::{Json, extract::State};

use crate::http::state::HttpState;

pub async fn validate_path(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn validate_mentions(
    State(_state): State<HttpState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn scroll_to_line(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}
