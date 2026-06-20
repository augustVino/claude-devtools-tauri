//! 核心 SSH 连接流程：TCP + Agent 发现 + 认证 + SFTP。
//!
//! 包含：
//! - establish_raw_connection(): 自由函数，connect()/test() 的共享逻辑
//! - build_connected_bundle(): 从 RawConnection 构建业务层 ConnectedBundle
//! - open_sftp_subsystem_static(): SFTP 子系统打开（内联，仅 ~15 行）

use std::sync::Arc;
use std::time::{Duration, Instant};

use russh::client;
use russh_sftp::client::SftpSession;

use crate::infrastructure::ssh_auth::{self, AuthError};
use crate::infrastructure::ssh_config_parser::SshConfigParser;
use crate::infrastructure::ssh_fs_provider::SshFsProvider;

use super::auth_trace::AuthTrace;
use super::tcp_probe;
use super::{ConnectRequest, ConnectedBundle, RawConnection, SshClientHandler};

/// Connection timeout (10 seconds).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// 核心：TCP 连接 + Agent 发现 + 认证 + SFTP 打开。
///
/// 自由函数（非 &self 方法），仅依赖传入参数，不访问 manager 状态。
/// 返回 RawConnection（裸资源），不创建 FsProvider、不解析路径、不存储状态、不发射事件。
///
/// Phase 3a: 返回 `Result<_, AuthError>`；建立 AuthTrace，TCP 预探测 + 各阶段计时；
/// 所有 Err 路径用 `AuthError::with_trace` 携带 trace，Display 时自动渲染为多段诊断信息。
pub(super) async fn establish_raw_connection(
    request: &ConnectRequest,
    config_parser: Option<&SshConfigParser>,
) -> Result<RawConnection, AuthError> {
    let mut trace = AuthTrace::new();

    // Phase 1: Merge (preserved as fallback path)
    let merged_config = super::ssh_config_merge::merge_with_ssh_config_static(
        request.config.clone(),
        config_parser,
    );
    if merged_config.host.trim().is_empty() {
        return Err(AuthError::new("Host is required"));
    }

    // Phase 1.5 (phase 2 new): ssh -G. Pass original_host (alias), not merged HostName.
    // Phase 3a: 计时 resolve_ms。
    let resolve_start = Instant::now();
    let resolved = super::host_resolver::resolve_host(&request.original_host, config_parser).await;
    trace.timings.resolve_ms = resolve_start.elapsed().as_millis() as u64;

    let mut final_config = merged_config.clone();
    if !resolved.hostname.is_empty() && resolved.hostname != final_config.host {
        final_config.host = resolved.hostname.clone();
    }
    if resolved.port != 22 && final_config.port == 22 {
        final_config.port = resolved.port;
    }
    if let Some(ref user) = resolved.user {
        if final_config.username.is_empty() {
            final_config.username = user.clone();
        }
    }

    // Phase 1.6 (phase 3a new): TCP pre-probe (timed).
    // 区分 "host unreachable"（per-app VPN 拦截）与 "auth rejected"，
    // 在 russh connect 可能挂起 CONNECT_TIMEOUT 之前给出快速、可诊断的失败。
    let probe_result = tcp_probe::probe_tcp(&final_config.host, final_config.port).await;
    trace.timings.tcp_probe_ms = probe_result.elapsed_ms();

    if !probe_result.is_reachable() {
        let root_msg = probe_result
            .diagnostic_message(&final_config.host, final_config.port)
            .unwrap_or_else(|| {
                format!(
                    "TCP probe to {}:{} failed",
                    final_config.host, final_config.port
                )
            });
        return Err(AuthError::with_trace(root_msg, trace));
    }

    // Phase 2: TCP + SSH handshake (timed; None if error during handshake)
    let handshake_start = Instant::now();
    let addr = (final_config.host.as_str(), final_config.port);
    let russh_config = Arc::new(russh::client::Config::default());
    let session = match tokio::time::timeout(
        CONNECT_TIMEOUT,
        russh::client::connect(russh_config, addr, SshClientHandler),
    )
    .await
    {
        Ok(Ok(h)) => {
            trace.timings.tcp_handshake_ms = Some(handshake_start.elapsed().as_millis() as u64);
            h
        }
        Ok(Err(e)) => {
            let root_msg = format!(
                "SSH connection to {}:{} failed: {}",
                final_config.host, final_config.port, e
            );
            return Err(AuthError::with_trace(root_msg, trace));
        }
        Err(_) => {
            let root_msg = format!(
                "SSH connection to {}:{} timed out after {}s",
                final_config.host,
                final_config.port,
                CONNECT_TIMEOUT.as_secs()
            );
            return Err(AuthError::with_trace(root_msg, trace));
        }
    };

    // Phase 3 (phase 2 new): multi-candidate agent discovery
    let agent_sockets = super::agent_discovery::discover_agent_sockets(
        resolved.identity_agent.as_deref().and_then(|p| p.to_str()),
    )
    .await;

    // Phase 4: Authenticate (pass trace for attempt collection)
    let mut session_mut = session;
    if let Err(auth_err) = ssh_auth::authenticate(
        &mut session_mut,
        &final_config.username,
        &final_config.auth_method,
        final_config.password.as_deref(),
        final_config.private_key_path.as_deref(),
        &resolved.identity_files,
        &agent_sockets,
        &mut trace,
    )
    .await
    {
        return Err(AuthError::with_trace(
            format!("authentication failed: {}", auth_err.message),
            trace,
        ));
    }

    // Phase 5: Open SFTP (8s timeout + IPv6-aware diagnostic)
    let sftp = match open_sftp_subsystem_static(
        &mut session_mut,
        &final_config.username,
        &final_config.host,
    )
    .await
    {
        Ok(s) => s,
        Err(msg) => return Err(AuthError::with_trace(msg, trace)),
    };

    Ok(RawConnection {
        merged_config: final_config,
        original_host: request.original_host.clone(),
        session: session_mut,
        sftp,
    })
}

/// 从 RawConnection 构建 ConnectedBundle（仅 connect() 调用路径）。
pub(super) async fn build_connected_bundle(
    request: ConnectRequest,
    mut raw: RawConnection,
) -> Result<ConnectedBundle, String> {
    let fs_provider = SshFsProvider::new(raw.sftp, tokio::runtime::Handle::current());
    let remote_projects_path = super::remote_path_resolver::resolve_remote_projects_path_static(
        &mut raw.session,
        &raw.merged_config.username,
        &fs_provider,
    )
    .await;
    let status = crate::types::ssh::SshConnectionStatus::connected(
        pick_status_host(&request),
        remote_projects_path.clone(),
    );

    Ok(ConnectedBundle {
        request,
        merged_config: raw.merged_config,
        original_host: raw.original_host,
        session: raw.session,
        fs_provider,
        remote_projects_path,
        status,
    })
}

/// 决定 SshConnectionStatus.host 的来源（对齐 Electron connectedHost = config.host）。
///
/// 独立纯函数便于单测，避免依赖 RawConnection（russh session 不可 mock）。
fn pick_status_host(request: &ConnectRequest) -> String {
    request.original_host.clone()
}

/// Open SFTP subsystem with 8s timeout + diagnostic.
///
/// Signature (phase 2): (session) → (session, user, host). Private (same-file caller only).
async fn open_sftp_subsystem_static(
    session: &mut client::Handle<SshClientHandler>,
    user: &str,
    host: &str,
) -> Result<SftpSession, String> {
    let open_future = async {
        let channel = session
            .channel_open_session()
            .await
            .map_err(|e| format!("Failed to open SSH session channel for SFTP: {}", e))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|e| format!("Failed to request SFTP subsystem: {}", e))?;
        let stream = channel.into_stream();
        SftpSession::new(stream)
            .await
            .map_err(|e| format!("Failed to initialize SFTP session: {}", e))
    };

    match tokio::time::timeout(ssh_auth::SFTP_OPEN_TIMEOUT, open_future).await {
        Ok(result) => result,
        Err(_) => Err(ssh_auth::build_sftp_timeout_msg(
            user,
            host,
            ssh_auth::SFTP_OPEN_TIMEOUT.as_secs(),
        )),
    }
}

#[cfg(test)]
mod tests_align_electron {
    use super::*;
    use crate::types::ssh::{SshAuthMethod, SshConnectionConfig};

    fn make_request(host: &str) -> ConnectRequest {
        let config = SshConnectionConfig {
            host: host.to_string(),
            port: 22,
            username: String::new(),
            auth_method: SshAuthMethod::Auto,
            password: None,
            private_key_path: None,
        };
        ConnectRequest::new(config)
    }

    /// 函数级测试：验证 pick_status_host 返回 original_host。
    ///
    /// **覆盖范围诚实声明**（v4-C3）：本测试只覆盖 pick_status_host 函数体逻辑，
    /// 不覆盖 build_connected_bundle 是否真的调用了 pick_status_host。
    /// 调用点正确性依赖：
    ///   - 代码审查（Step 5 改动）
    ///   - SSE 契约测试不破坏（Step 9）
    ///   - 手动验证（Step 13）
    /// 完整调用点覆盖需 russh session mock（独立工作）。
    #[test]
    fn test_pick_status_host_returns_original_host() {
        let request = make_request("myserver");
        assert_eq!(pick_status_host(&request), "myserver");
    }

    #[test]
    fn test_pick_status_host_preserves_dotted_alias() {
        let request = make_request("my-server.example.com");
        assert_eq!(pick_status_host(&request), "my-server.example.com");
    }
}
