//! HTTP 服务器模块。
//!
//! 提供 Axum HTTP 服务器，用于浏览器访问会话可视化功能。
//! 与 Tauri IPC 命令共享 Arc<RwLock<AppState>>。

pub mod cors;
pub mod routes;
pub mod sse;
pub mod server;
pub mod state;

use std::path::PathBuf;

use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use tower_http::services::{ServeDir, ServeFile};

use crate::http::state::HttpState;

/// 未匹配 /api/* 路径时返回 JSON 404，避免 fallback 返回 HTML。
async fn api_404(req: Request) -> Response {
    let path = req.uri().path();
    if path.starts_with("/api/") {
        (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"success":false,"error":"not found"}"#,
        )
            .into_response()
    } else {
        // 非 /api/* 路径：交给下一层 fallback 处理
        // 返回 404 让 ServeDir 的 not_found_service 处理
        (StatusCode::NOT_FOUND, "").into_response()
    }
}

/// 构建 Axum 路由。
///
/// `dist_dir`: 前端构建产物目录的绝对路径（如 `/path/to/project/dist`）。
pub fn build_router(http_state: HttpState, dist_dir: PathBuf) -> Router {
    let api_routes = routes::build_routes();

    let index_html = dist_dir.join("index.html");
    let static_files = ServeDir::new(&dist_dir)
        .not_found_service(ServeFile::new(index_html));

    Router::new()
        .merge(api_routes)
        .fallback(api_404)
        .fallback_service(static_files)
        .layer(cors::cors_layer())
        .with_state(http_state)
}
