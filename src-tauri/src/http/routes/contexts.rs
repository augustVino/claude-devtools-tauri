//! 上下文切换 HTTP 路由。
//!
//! 对应 Tauri 命令：context.rs 中的上下文管理命令。

use axum::{
    Json,
    extract::State,
    routing::{get, post},
};
use serde::Deserialize;
use tauri::Manager;

use crate::http::state::HttpState;
use crate::http::sse::BackendEvent;
use crate::infrastructure::context_manager::{ContextInfo, SwitchResponse};

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
) -> Result<Json<SwitchResponse>, (axum::http::StatusCode, Json<ErrorResponse>)> {
    let context_id = body.context_id.trim();
    if context_id.is_empty() || context_id.len() > 256 {
        return Err(error_json(String::from("Invalid context_id")));
    }

    let mut mgr = state.context_manager.write().await;

    // Use switch_with_watcher_actions (Batch 1 Task 1)
    let (result, actions) = mgr.switch_with_watcher_actions(context_id)
        .map_err(|e| error_json(e.to_string()))?;

    // Execute watcher lifecycle based on actions
    if actions.should_stop_old || actions.should_start_new {
        let cm = state.app_handle
            .state::<std::sync::Arc<crate::infrastructure::ConfigManager>>()
            .inner()
            .clone();
        let nm = state.app_handle
            .state::<std::sync::Arc<tokio::sync::RwLock<crate::infrastructure::NotificationManager>>>()
            .inner()
            .clone();
        if actions.should_stop_old {
            if let Some(old_ctx) = mgr.get(&actions.old_context_id) {
                old_ctx.read().await.stop_watcher_tasks().await;
            }
        }
        if actions.should_start_new {
            if let Some(new_ctx) = mgr.get(&actions.new_context_id) {
                let new = new_ctx.read().await;
                new.spawn_watcher_tasks(state.app_handle.clone(), cm, nm).await;
            }
        }
    }

    // Bug B3 fix: drop write lock BEFORE SSE send
    if result.previous_id != result.current_id {
        let new_ctx = mgr.get(&result.current_id).unwrap();
        let info = ContextInfo::from_context(&*new_ctx.read().await);
        drop(mgr);  // ← BUG FIX: release lock before broadcast
        state.broadcaster.send(BackendEvent::ContextChanged(info));
    } else {
        drop(mgr);
    }

    Ok(Json(SwitchResponse { context_id: result.current_id }))
}

/// 构建上下文路由。
pub fn routes() -> axum::Router<HttpState> {
    axum::Router::new()
        .route("/api/contexts", get(context_list))
        .route("/api/contexts/active", get(context_active))
        .route("/api/contexts/switch", post(context_switch))
}
