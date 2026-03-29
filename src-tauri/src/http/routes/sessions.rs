//! Sessions 路由处理器（占位）。

use axum::{Json, extract::State};

use crate::http::state::HttpState;

pub async fn get_sessions(State(_state): State<HttpState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn get_sessions_paginated(
    State(_state): State<HttpState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn get_sessions_by_ids(
    State(_state): State<HttpState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn get_session_detail(
    State(_state): State<HttpState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn get_session_groups(
    State(_state): State<HttpState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn get_session_metrics(
    State(_state): State<HttpState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}

pub async fn get_waterfall_data(
    State(_state): State<HttpState>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"error": "TODO"}))
}
