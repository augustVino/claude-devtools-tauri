//! HTTP 服务器模块。
//!
//! 提供 Axum HTTP 服务器，用于浏览器访问会话可视化功能。
//! 与 Tauri IPC 命令共享 Arc<RwLock<AppState>>。

pub mod cors;
pub mod path_validation;
pub mod routes;
pub mod server;
pub mod sse;
pub mod state;

use std::path::PathBuf;

use axum::{
    http::{header, StatusCode},
    response::IntoResponse,
    routing::{any_service, get},
    Router,
};
use tower_http::services::{ServeDir, ServeFile};

use crate::http::state::HttpState;

/// 构建 Axum 路由。
///
/// `dist_dir`: 前端构建产物目录的绝对路径（如 `/path/to/project/dist`）。
pub fn build_router(http_state: HttpState, dist_dir: PathBuf) -> Router {
    let api_routes = routes::build_routes();

    let dist = dist_dir.clone();
    let index_html = dist.join("index.html");
    let spa_index = index_html.clone();
    let serve_dir = ServeDir::new(&dist_dir).not_found_service(ServeFile::new(index_html));
    Router::new()
        .merge(api_routes)
        .route(
            "/",
            get(move || async move { spa_handler(&spa_index).await }),
        )
        .fallback_service(any_service(serve_dir))
        .layer(cors::cors_layer())
        .with_state(http_state)
}

/// SPA index.html 保底 handler：读取并返回 dist/index.html。
async fn spa_handler(index_path: &std::path::Path) -> impl IntoResponse {
    match tokio::fs::read_to_string(index_path).await {
        Ok(html) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            html,
        )
            .into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain")],
            format!("SPA index not found: {}", e),
        )
            .into_response(),
    }
}
