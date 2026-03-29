//! 上下文切换 HTTP 路由。
//!
//! 对应 Tauri 命令：context.rs 中的上下文管理命令。

use axum::{
    Json,
    extract::State,
    routing::{get, post},
};
use serde::Deserialize;

use crate::http::state::HttpState;
use crate::http::sse::BackendEvent;
use crate::infrastructure::context_manager::ContextInfo;

use super::{ErrorResponse, error_json};

/// 切换上下文请求体。
#[derive(Deserialize)]
pub struct SwitchRequest {
    #[serde(rename = "contextId")]
    pub context_id: String,
}

/// GET /api/contexts — 列出所有已注册的上下文。
pub async fn context_list(
    State(state): State<HttpState>,
) -> Json<Vec<ContextInfo>> {
    Json(state.context_manager.read().await.list())
}

/// GET /api/contexts/active — 获取当前活跃上下文 ID。
pub async fn context_active(
    State(state): State<HttpState>,
) -> Json<String> {
    Json(state.context_manager.read().await.get_active_id().to_string())
}

/// POST /api/contexts/switch — 切换到指定上下文。
pub async fn context_switch(
    State(state): State<HttpState>,
    Json(body): Json<SwitchRequest>,
) -> Result<Json<String>, (axum::http::StatusCode, Json<ErrorResponse>)> {
    let mut mgr = state.context_manager.write().await;
    let result = mgr.switch(&body.context_id)
        .map_err(error_json)?;

    // Stop old context watcher tasks
    if let Some(old_ctx) = mgr.get(&result.previous_id) {
        old_ctx.read().await.stop_watcher_tasks();
    }

    // Note: HTTP routes don't have AppHandle, so we can't spawn watcher tasks here.
    // Watcher lifecycle is managed by Tauri IPC commands.
    // TODO: When SSH is implemented, consider adding AppHandle to HttpState.

    let new_ctx = mgr.get(&result.current_id).unwrap();
    let info = ContextInfo::from_context(&*new_ctx.read().await);

    // Bridge to SSE for HTTP clients
    state.broadcaster.send(BackendEvent::ContextChanged(info));

    Ok(Json(result.current_id))
}

/// 构建上下文路由。
pub fn routes() -> axum::Router<HttpState> {
    axum::Router::new()
        .route("/api/contexts", get(context_list))
        .route("/api/contexts/active", get(context_active))
        .route("/api/contexts/switch", post(context_switch))
}
