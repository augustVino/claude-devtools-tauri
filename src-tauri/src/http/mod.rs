//! HTTP 服务器模块。
//!
//! 提供 Axum HTTP 服务器，用于浏览器访问会话可视化功能。
//! 与 Tauri IPC 命令共享 Arc<RwLock<AppState>>。

pub mod cors;
pub mod routes;
pub mod sse;
pub mod server;
pub mod state;

use axum::Router;

use crate::http::state::HttpState;

/// 构建 Axum 路由。
pub fn build_router(http_state: HttpState) -> Router {
    Router::new()
        .merge(routes::build_routes())
        .layer(cors::cors_layer())
        .with_state(http_state)
}
