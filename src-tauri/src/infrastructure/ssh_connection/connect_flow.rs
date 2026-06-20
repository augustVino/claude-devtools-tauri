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

/// Merge resolved `ssh -G` values into final_config (pure function for testing).
///
/// 对齐 Electron `resolveTarget` 的字段合并逻辑（SshConnectionManager.ts:433-436）。
/// - hostname: 仅当 resolved 非空且与 final_config.host 不同时覆盖
/// - port: 仅当非 fallback（ssh -G 真实解析成功）时覆盖；fallback 路径
///   保留 final_config.port（保护 ssh_config_merge 已填的 entry.port）
/// - username: 仅当 final_config.username 为空时填
pub(super) fn merge_resolved_into_config(
    final_config: &mut crate::types::ssh::SshConnectionConfig,
    resolved: &super::host_resolver::ResolvedHost,
) {
    if !resolved.hostname.is_empty() && resolved.hostname != final_config.host {
        final_config.host = resolved.hostname.clone();
    }
    // 用显式 was_fallback 标志判断（不用 hostname 比较，避免巧合相等误判）
    if !resolved.was_fallback {
        final_config.port = resolved.port;
    }
    if let Some(ref user) = resolved.user {
        if final_config.username.is_empty() {
            final_config.username = user.clone();
        }
    }
}

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
    merge_resolved_into_config(&mut final_config, &resolved);

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

    #[test]
    fn test_merge_resolved_port_overrides_config_port_unconditionally() {
        // Electron 对齐：非 fallback 路径下 resolved.port 总是覆盖 config.port
        // 参考 SshConnectionManager.ts:435 `port: resolved?.port ?? config.port`
        let mut config = make_request("myserver").config;
        config.port = 2222; // 用户显式指定（或 ssh_config_merge 填充）
        let resolved = super::super::host_resolver::ResolvedHost {
            hostname: "1.2.3.4".to_string(), // 非 fallback
            port: 22, // ssh -G 返回 22
            user: None,
            identity_files: vec![],
            identity_agent: None,
            was_fallback: false, // ssh -G 真实解析成功
        };
        merge_resolved_into_config(&mut config, &resolved);
        assert_eq!(config.port, 22, "resolved.port must override (Electron parity)");
        assert_eq!(config.host, "1.2.3.4");
    }

    #[test]
    fn test_merge_resolved_custom_port_when_config_default() {
        let mut config = make_request("myserver").config;
        let resolved = super::super::host_resolver::ResolvedHost {
            hostname: "1.2.3.4".to_string(),
            port: 2222,
            user: Some("deploy".to_string()),
            identity_files: vec![],
            identity_agent: None,
            was_fallback: false,
        };
        merge_resolved_into_config(&mut config, &resolved);
        assert_eq!(config.port, 2222);
        assert_eq!(config.host, "1.2.3.4");
        assert_eq!(config.username, "deploy");
    }

    #[test]
    fn test_merge_resolved_skips_empty_hostname() {
        let mut config = make_request("myserver").config;
        let original_host = config.host.clone();
        let resolved = super::super::host_resolver::ResolvedHost {
            hostname: String::new(),
            port: 22,
            user: None,
            identity_files: vec![],
            identity_agent: None,
            was_fallback: false,
        };
        merge_resolved_into_config(&mut config, &resolved);
        assert_eq!(config.host, original_host, "empty hostname must not overwrite");
    }

    #[test]
    fn test_merge_resolved_preserves_merged_port_on_fallback() {
        // 修正 codex 第二轮 C2 + 自我审查缺陷：
        // fallback 路径下保留 ssh_config_merge 已填的 port
        let mut config = make_request("myalias").config;
        config.port = 2222; // 模拟 ssh_config_merge 已填的 entry.port
        let resolved = super::super::host_resolver::ResolvedHost {
            hostname: "myalias".to_string(), // fallback 时 hostname == input_host
            port: 22, // fallback port
            user: None,
            identity_files: vec![],
            identity_agent: None,
            was_fallback: true, // 显式标记 fallback
        };
        merge_resolved_into_config(&mut config, &resolved);
        assert_eq!(config.port, 2222, "fallback path must preserve merged_config.port");
    }

    #[test]
    fn test_merge_resolved_overrides_port_when_ssh_g_succeeds_with_same_hostname() {
        // 关键回归测试：用户输入 IP `1.2.3.4`，ssh -G 真实解析成功返回相同 hostname
        // 此时 was_fallback=false（不能用 hostname 比较判断），应该正常覆盖 port
        let mut config = make_request("1.2.3.4").config;
        config.port = 2222; // ssh_config 给该 IP 定义了 Port 2222
        let resolved = super::super::host_resolver::ResolvedHost {
            hostname: "1.2.3.4".to_string(), // 与 input 相同（但非 fallback！）
            port: 22, // ssh -G 真实返回 22
            user: None,
            identity_files: vec![],
            identity_agent: None,
            was_fallback: false, // 真实解析成功
        };
        merge_resolved_into_config(&mut config, &resolved);
        // 关键：ssh -G 真实解析的 22 应该覆盖 ssh_config 的 2222
        assert_eq!(
            config.port, 22,
            "real ssh -G must override ssh_config port even when hostname equals input"
        );
    }
}
