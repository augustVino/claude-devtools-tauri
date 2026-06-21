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

/// Connection timeout (15 seconds) — covers TCP + SSH handshake only.
/// Auth 和 SFTP open 有各自的独立 timeout（ssh_auth::AUTH_TIMEOUT / SFTP_OPEN_TIMEOUT）。
/// 对齐 Electron SSH2_READY_TIMEOUT_MS=22s（含 headroom），慢速 VPN/高延迟 host 不再过早 timeout。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Outer hard-cap timeout (25 seconds) for the whole connect chain
/// (handshake + multi-candidate auth + SFTP open).
/// 对齐 Electron CONNECT_TIMEOUT_MS=25s outer race（SshConnectionManager.ts:82）。
///
/// ## Engineering trade-off: disconnect on timeout
///
/// **目标对齐**：Electron `client.end()`（ssh2 库）— timeout 触发时主动断链。
///
/// **当前实现**：timeout 触发时整个 connect future 被 drop，session Handle 经 Drop 释放。
/// **不调用 `Handle::disconnect()`**，原因：
/// 1. `Handle::disconnect(&self, ...)` 与 SFTP open 的 `&mut session` 借用冲突
///    （`session_mut` 已 move 进 async block，timeout 分支不可访问）
/// 2. russh 0.46 的 `Handle` 不实现 `Clone`（核实：`pub struct Handle<H> { sender, receiver: UnboundedReceiver, join }`，
///    `UnboundedReceiver` 不 Clone，整个 struct 无 `#[derive(Clone)]`。
///    源码位置：russh-0.46.0/src/client/mod.rs:221-225）
/// 3. 即便强行 `Arc<OnceCell<Handle>>`，phase 2-5 内 `&mut self` 借用期间无法 set
///
/// **实际副作用**：
/// - russh 0.46 `Handle::drop` 仅 log，**不发送 SSH disconnect 消息**
///   （核实：`impl<H> Drop for Handle<H> { fn drop(&mut self) { debug!("drop handle") } }`
///   源码位置：russh-0.46.0/src/client/mod.rs:227-231）
/// - 远端 sshd 的 socket 进入 `TIME_WAIT` / `CLOSE_WAIT`，等待 OS TCP keepalive 清理
/// - 默认 Linux TCP keepalive：2 小时后开始 probe，9 次 probe 各 75s → **最多 ~2h15min 才彻底回收**
/// - 用户感知：连接已 failed，但 `ss -tn | grep :22` 仍能看到 stale 连接
///
/// **缓解措施**：
/// - 在 connect 路径每个阶段都用了独立 timeout（CONNECT/AUTH/SFTP_OPEN）
/// - outer race 25s 是绝对硬上限，绝不会无限挂起
/// - server 端 sshd 配置 `ClientAliveInterval` 通常更短（默认 0=禁用，但运维常设 60-300s）
///
/// **未来改进路径**（需重大重构）：
/// - 升级 russh 0.50+（若 release）— 检查是否支持 `Handle::disconnect` 与 SFTP 并发
/// - 或重写 connect 流程为 state machine，session Handle 在 timeout 分支独立可达
///
/// **Reviewer 评估**：非阻塞，不影响功能正确性，仅影响 stale socket 清理速度。
const CONNECT_CHAIN_TIMEOUT: Duration = Duration::from_secs(25);

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

    // Phase 2-5 wrapped in CONNECT_CHAIN_TIMEOUT outer race (25s hard cap).
    // 对齐 Electron SshConnectionManager.ts:370-403 outer race 语义：
    // 内层 CONNECT_TIMEOUT/AUTH_TIMEOUT/SFTP_OPEN_TIMEOUT 各阶段独立，
    // outer race 包整链防止 worst-case 累积超过 25s。
    //
    // block 返回 Result<_, String>（不 take trace ownership），外层统一包装 trace，
    // 这样 timeout 分支也能复用 trace。
    //
    // 注意：timeout 触发时整个 future 被 drop，session Handle 经 Drop 释放。
    // russh 0.46 的 Handle::drop 不主动 disconnect（仅 log），socket 不立即关闭，
    // 依赖 OS TCP keepalive 最终清理。非严格等价 Electron client.end()，工程妥协。
    let chain_result =
        tokio::time::timeout(CONNECT_CHAIN_TIMEOUT, async {
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
                    trace.timings.tcp_handshake_ms =
                        Some(handshake_start.elapsed().as_millis() as u64);
                    h
                }
                Ok(Err(e)) => {
                    return Err(format!(
                        "SSH connection to {}:{} failed: {}",
                        final_config.host, final_config.port, e
                    ));
                }
                Err(_) => {
                    return Err(format!(
                        "SSH connection to {}:{} timed out after {}s",
                        final_config.host,
                        final_config.port,
                        CONNECT_TIMEOUT.as_secs()
                    ));
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
                return Err(format!("authentication failed: {}", auth_err.message));
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
                Err(msg) => return Err(msg),
            };

            Ok((session_mut, sftp))
        })
        .await;

    match chain_result {
        Ok(Ok((session_mut, sftp))) => Ok(RawConnection {
            merged_config: final_config,
            original_host: request.original_host.clone(),
            session: session_mut,
            sftp,
        }),
        Ok(Err(root_msg)) => Err(AuthError::with_trace(root_msg, trace)),
        Err(_) => {
            // outer race timeout (25s) — 对齐 Electron enrichAuthError 风格
            let root_msg = format!(
                "SSH connection chain to {}:{} timed out after {}s (handshake + auth + SFTP). \
                 Inner timeouts: connect={}s, auth={}s, sftp_open={}s.",
                final_config.host,
                final_config.port,
                CONNECT_CHAIN_TIMEOUT.as_secs(),
                CONNECT_TIMEOUT.as_secs(),
                ssh_auth::AUTH_TIMEOUT.as_secs(),
                ssh_auth::SFTP_OPEN_TIMEOUT.as_secs(),
            );
            Err(AuthError::with_trace(root_msg, trace))
        }
    }
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
