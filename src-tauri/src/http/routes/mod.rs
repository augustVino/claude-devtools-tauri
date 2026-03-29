//! HTTP 路由聚合模块（临时最小实现，Task 4 将替换）。

use axum::Router;
use crate::http::state::HttpState;

pub fn build_routes() -> Router<HttpState> {
    Router::new()
}
