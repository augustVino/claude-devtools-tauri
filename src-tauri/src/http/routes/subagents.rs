//! Subagents 路由处理器（占位）。

use axum::{Json, extract::State};

use crate::http::state::HttpState;

pub async fn get_subagent_detail(
    State(_state): State<HttpState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}
