//! Projects 路由处理器（占位）。

use axum::{Json, extract::State};

use crate::http::state::HttpState;

use super::error_json;

pub async fn get_projects(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn get_repository_groups(
    State(_state): State<HttpState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn get_worktree_sessions(
    State(_state): State<HttpState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}
