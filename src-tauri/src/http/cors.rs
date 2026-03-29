//! CORS 配置 — 仅允许 localhost 访问。

use axum::http::Method;
use tower_http::cors::{AllowOrigin, CorsLayer};

/// 创建 localhost-only CORS 层。
pub fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin, _request_parts| {
            let origin_str = match origin.to_str() {
                Ok(s) => s,
                Err(_) => return false,
            };

            // 精确匹配
            if matches!(
                origin_str,
                "http://localhost"
                    | "http://127.0.0.1"
                    | "https://localhost"
                    | "https://127.0.0.1"
            ) {
                return true;
            }

            // 前缀匹配（带端口号）
            origin_str.starts_with("http://localhost:")
                || origin_str.starts_with("http://127.0.0.1:")
                || origin_str.starts_with("https://localhost:")
                || origin_str.starts_with("https://127.0.0.1:")
        }))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
        ])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::ACCEPT,
        ])
}
