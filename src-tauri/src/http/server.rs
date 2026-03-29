//! HTTP 服务器生命周期管理。
//!
//! 负责 Axum 服务器的启动、关闭和端口选择。

use std::path::PathBuf;

use tokio_util::sync::CancellationToken;

use crate::http::state::HttpState;

/// HTTP 服务器运行状态。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpServerStatus {
    pub running: bool,
    pub port: u16,
}

/// HTTP 服务器句柄 — 持有 shutdown token 和端口信息。
pub struct HttpServerHandle {
    pub shutdown: CancellationToken,
    pub port: u16,
}

/// 在 3456-3466 范围内寻找可用端口。
fn find_available_port(start: u16) -> Option<u16> {
    for port in start..=start + 10 {
        if std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).is_ok() {
            return Some(port);
        }
    }
    None
}

/// 启动 Axum HTTP 服务器。
///
/// 在后台 tokio task 中运行，绑定到 127.0.0.1。
pub fn spawn_http_server(
    http_state: HttpState,
    preferred_port: u16,
    dist_dir: PathBuf,
) -> Result<HttpServerHandle, String> {
    let port = find_available_port(preferred_port)
        .ok_or_else(|| format!("No available port in range {}-{}", preferred_port, preferred_port + 10))?;

    let shutdown = CancellationToken::new();

    let app = crate::http::build_router(http_state, dist_dir);
    let shutdown_token = shutdown.clone();

    tokio::spawn(async move {
        match tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port)).await {
            Ok(listener) => {
                log::info!("HTTP server listening on 127.0.0.1:{}", port);
                if let Err(e) = axum::serve(listener, app)
                    .with_graceful_shutdown(async move { shutdown_token.cancelled().await; })
                    .await
                {
                    log::error!("HTTP server error: {}", e);
                }
            }
            Err(e) => {
                log::error!("Failed to bind port {}: {}", port, e);
            }
        }
    });

    Ok(HttpServerHandle { shutdown, port })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_available_port_returns_valid_port() {
        let port = find_available_port(3456);
        assert!(port.is_some());
        assert!(port.unwrap() >= 3456);
        assert!(port.unwrap() <= 3466);
    }

    #[test]
    fn find_available_port_binds() {
        let port = find_available_port(3456).unwrap();
        // Verify we can actually bind to it
        let listener = std::net::TcpListener::bind(format!("127.0.0.1:{}", port));
        assert!(listener.is_ok());
    }
}
