//! CORS 配置 — 支持 localhost 默认模式、CORS_ORIGIN=* 通配模式和自定义 origin 列表。

use axum::http::Method;
use tower_http::cors::{AllowOrigin, CorsLayer};

/// 构建 CORS 层。
///
/// 三种模式（通过 `CORS_ORIGIN` 环境变量控制）：
/// 1. 未设置或空 → 仅允许 localhost / 127.0.0.1
/// 2. `CORS_ORIGIN=*` → 允许所有 origin
/// 3. `CORS_ORIGIN=host1,host2,...` → 允许指定的 origin 列表
pub fn cors_layer() -> CorsLayer {
    let cors_origin = std::env::var("CORS_ORIGIN").unwrap_or_default();

    let allow_origin = if cors_origin == "*" {
        AllowOrigin::predicate(|_origin, _: &_| true)
    } else if cors_origin.is_empty() {
        // Default: localhost only
        AllowOrigin::predicate(|origin, _request_parts| {
            let origin_str = match origin.to_str() {
                Ok(s) => s,
                Err(_) => return true,
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
        })
    } else {
        // Specific origins list
        let allowed: Vec<String> = cors_origin
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if allowed.is_empty() {
            log::warn!("CORS_ORIGIN is set but parsed to empty list; all cross-origin requests will be denied");
        }
        AllowOrigin::predicate(move |origin, _request_parts| {
            let origin_str = match origin.to_str() {
                Ok(s) => s,
                Err(_) => return true,
            };
            allowed.iter().any(|a| a == origin_str)
        })
    };

    CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_credentials(true)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
        ])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::ACCEPT,
            axum::http::header::AUTHORIZATION,
        ])
}
