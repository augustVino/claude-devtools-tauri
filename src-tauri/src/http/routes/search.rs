//! Search 路由处理器（占位）。

use axum::{Json, extract::State};

use crate::http::state::HttpState;

pub async fn search_sessions(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn search_all_projects(
    State(_state): State<HttpState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}
