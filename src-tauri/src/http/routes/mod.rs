//! HTTP 路由聚合模块。
//!
//! 将所有 REST 端点注册到 Axum Router。

use axum::{
    Json,
    Router,
    extract::{Path as AxumPath, State},
    http::StatusCode,
    routing::{delete, get, post, put},
};

use crate::http::state::HttpState;

use serde::Serialize;

pub mod config;
pub mod contexts;
pub mod events;
pub mod notifications;
pub mod projects;
pub mod search;
pub mod sessions;
pub mod ssh;
pub mod subagents;
pub mod utility;
pub mod validation;

/// 通用错误响应。
#[derive(Serialize)]
pub(crate) struct ErrorResponse {
    pub success: bool,
    pub error: String,
}

/// 通用成功响应。
#[derive(Serialize)]
pub(crate) struct SuccessResponse {
    pub success: bool,
}

pub(crate) fn error_json(msg: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::OK,
        Json(ErrorResponse {
            success: false,
            error: msg.into(),
        }),
    )
}

pub(crate) fn success_response() -> (StatusCode, Json<SuccessResponse>) {
    (
        StatusCode::OK,
        Json(SuccessResponse { success: true }),
    )
}

/// "Not available in browser mode" 响应 — 对齐 Electron HTTP no-op 语义。
pub(crate) fn not_available_response() -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::OK,
        Json(ErrorResponse {
            success: false,
            error: "Not available in browser mode".to_string(),
        }),
    )
}

/// 带数据的成功响应 — 序列化为 { "success": true, "data": <T> }。
pub(crate) fn success_json<T: Serialize>(data: T) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::OK,
        Json(serde_json::json!({ "success": true, "data": data })),
    )
}

pub(crate) fn not_implemented() -> (StatusCode, Json<ErrorResponse>) {
    error_json("not implemented")
}

/// 构建所有路由。
///
/// 注意：Axum Router 只支持单一 State 类型，因此使用合并的 `HttpState`。
/// 不需要在此处调用 `.with_state()`，由 `http::build_router()` 统一处理。
pub fn build_routes() -> Router<HttpState> {
    let mut router = Router::new()
        // Projects
        .route("/api/projects", get(projects::get_projects))
        .route(
            "/api/repository-groups",
            get(projects::get_repository_groups),
        )
        .route(
            "/api/worktrees/{id}/sessions",
            get(projects::get_worktree_sessions),
        )
        // Sessions
        .route(
            "/api/projects/{project_id}/sessions",
            get(sessions::get_sessions),
        )
        .route(
            "/api/projects/{project_id}/sessions-paginated",
            get(sessions::get_sessions_paginated),
        )
        .route(
            "/api/projects/{project_id}/sessions-by-ids",
            post(sessions::get_sessions_by_ids),
        )
        .route(
            "/api/projects/{project_id}/sessions/{session_id}",
            get(sessions::get_session_detail),
        )
        .route(
            "/api/projects/{project_id}/sessions/{session_id}/groups",
            get(sessions::get_session_groups),
        )
        .route(
            "/api/projects/{project_id}/sessions/{session_id}/metrics",
            get(sessions::get_session_metrics),
        )
        .route(
            "/api/projects/{project_id}/sessions/{session_id}/waterfall",
            get(sessions::get_waterfall_data),
        )
        // Subagents
        .route(
            "/api/projects/{project_id}/sessions/{session_id}/subagents/{subagent_id}",
            get(subagents::get_subagent_detail),
        )
        // Search
        .route(
            "/api/projects/{project_id}/search",
            get(search::search_sessions),
        )
        .route("/api/search", get(search::search_all_projects))
        // Session ID lookup
        .route(
            "/api/sessions/{session_id}/locate",
            get(search::find_session_by_id),
        )
        .route(
            "/api/sessions/search-by-id/{fragment}",
            get(search::find_sessions_by_partial_id),
        )
        // Config
        .route("/api/config", get(config::get_config))
        .route("/api/config/update", post(config::update_config))
        .route(
            "/api/config/ignore-regex",
            post(config::add_ignore_regex).delete(config::remove_ignore_regex),
        )
        .route(
            "/api/config/ignore-repository",
            post(config::add_ignore_repository).delete(config::remove_ignore_repository),
        )
        .route("/api/config/snooze", post(config::snooze))
        .route("/api/config/clear-snooze", post(config::clear_snooze))
        .route(
            "/api/config/triggers",
            get(config::get_triggers).post(config::add_trigger),
        )
        .route(
            "/api/config/triggers/{trigger_id}",
            put(config::update_trigger).delete(config::remove_trigger),
        )
        .route(
            "/api/config/triggers/{trigger_id}/test",
            post(config::test_trigger),
        )
        .route("/api/config/pin-session", post(config::pin_session))
        .route("/api/config/unpin-session", post(config::unpin_session))
        .route("/api/config/hide-session", post(config::hide_session))
        .route("/api/config/unhide-session", post(config::unhide_session))
        .route("/api/config/hide-sessions", post(config::hide_sessions))
        .route("/api/config/unhide-sessions", post(config::unhide_sessions))
        .route("/api/config/select-folders", post(config::no_op))
        .route("/api/config/open-in-editor", post(config::no_op))
        // Notifications
        .route(
            "/api/notifications",
            get(notifications::get_notifications).delete(notifications::clear_notifications),
        )
        .route(
            "/api/notifications/{id}/read",
            post(notifications::mark_read),
        )
        .route(
            "/api/notifications/read-all",
            post(notifications::mark_all_read),
        )
        .route(
            "/api/notifications/{id}",
            delete(notifications::delete_notification),
        )
        .route(
            "/api/notifications/unread-count",
            get(notifications::get_unread_count),
        )
        .route(
            "/api/notifications/stats",
            get(notifications::get_stats),
        )
        // Validation
        .route("/api/validate/path", post(validation::validate_path))
        .route("/api/validate/mentions", post(validation::validate_mentions))
        .route("/api/session/scroll-to-line", post(validation::scroll_to_line))
        // Utility
        .route("/api/version", get(utility::get_version))
        .route("/api/read-claude-md", post(utility::read_claude_md))
        .route(
            "/api/read-directory-claude-md",
            post(utility::read_directory_claude_md),
        )
        .route(
            "/api/read-mentioned-file",
            post(utility::read_mentioned_file),
        )
        .route("/api/read-agent-configs", post(utility::read_agent_configs))
        .route("/api/open-path", post(utility::no_op))
        .route("/api/open-external", post(utility::no_op))
        // SSE
        .route("/api/events", get(events::sse_handler))
        // Context Switch
        .merge(contexts::routes())
        // SSH
        .merge(ssh::routes())
        // Deferred: Updater
        .route("/api/updater/check", post(deferred_not_implemented))
        .route("/api/updater/download", post(deferred_not_implemented))
        .route("/api/updater/install", post(deferred_not_implemented));

    // Catch-all for unmatched /api/ routes — return JSON 404 for any HTTP method
    router = router.route(
        "/api/{*rest}",
        axum::routing::any(|AxumPath(_rest): AxumPath<String>| async {
            (
                StatusCode::NOT_FOUND,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                "{\"error\":\"Not found\"}",
            )
        }),
    );

    router
}

/// SSH/updater deferred stub handler.
async fn deferred_not_implemented() -> (StatusCode, Json<ErrorResponse>) {
    not_implemented()
}
