//! 核心 SSH 连接流程：TCP + Agent 发现 + 认证 + SFTP。
//!
//! 包含：
//! - establish_raw_connection(): 自由函数，connect()/test() 的共享逻辑
//! - build_connected_bundle(): 从 RawConnection 构建业务层 ConnectedBundle
//! - open_sftp_subsystem_static(): SFTP 子系统打开（内联，仅 ~15 行）

use std::sync::Arc;
use std::time::Duration;

use russh::client;
use russh_sftp::client::SftpSession;

use crate::infrastructure::ssh_auth;
use crate::infrastructure::ssh_config_parser::SshConfigParser;
use crate::infrastructure::ssh_fs_provider::SshFsProvider;

use super::{ConnectRequest, RawConnection, ConnectedBundle, SshClientHandler};

/// Connection timeout (10 seconds).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// 核心：TCP 连接 + Agent 发现 + 认证 + SFTP 打开。
///
/// 自由函数（非 &self 方法），仅依赖传入参数，不访问 manager 状态。
/// 返回 RawConnection（裸资源），不创建 FsProvider、不解析路径、不存储状态、不发射事件。
pub(super) async fn establish_raw_connection(
    request: &ConnectRequest,
    config_parser: Option<&SshConfigParser>,
) -> Result<RawConnection, String> {
    // Phase 1: Merge (preserved as fallback path)
    let merged_config = super::ssh_config_merge::merge_with_ssh_config_static(
        request.config.clone(),
        config_parser,
    );
    if merged_config.host.trim().is_empty() {
        return Err("Host is required".into());
    }

    // Phase 1.5 (phase 2 new): ssh -G. Pass original_host (alias), not merged HostName.
    let resolved = super::host_resolver::resolve_host(
        &request.original_host,
        config_parser,
    ).await;
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

    // Phase 2: TCP Connect (10s)
    let addr = (final_config.host.as_str(), final_config.port);
    let russh_config = Arc::new(russh::client::Config::default());
    let session = match tokio::time::timeout(CONNECT_TIMEOUT, russh::client::connect(russh_config, addr, SshClientHandler)).await {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => return Err(format!(
            "SSH connection to {}:{} failed: {}",
            final_config.host, final_config.port, e
        )),
        Err(_) => return Err(format!(
            "SSH connection to {}:{} timed out after {}s",
            final_config.host, final_config.port, CONNECT_TIMEOUT.as_secs()
        )),
    };

    // Phase 3 (phase 2 new): multi-candidate agent discovery
    let agent_sockets = super::agent_discovery::discover_agent_sockets(
        resolved.identity_agent.as_deref().and_then(|p| p.to_str()),
    ).await;

    // Phase 4: Authenticate (new signature)
    let mut session_mut = session;
    if let Err(e) = ssh_auth::authenticate(
        &mut session_mut,
        &final_config.username,
        &final_config.auth_method,
        final_config.password.as_deref(),
        final_config.private_key_path.as_deref(),
        &resolved.identity_files,
        &agent_sockets,
    ).await {
        return Err(format!("authentication failed: {}", e));
    }

    // Phase 5: Open SFTP (8s timeout + IPv6-aware diagnostic)
    let sftp = open_sftp_subsystem_static(
        &mut session_mut,
        &final_config.username,
        &final_config.host,
    ).await?;

    Ok(RawConnection {
        merged_config: final_config,
        original_host: request.original_host.clone(),
        session: session_mut,
        sftp,
    })
}

/// 从 RawConnection 构建 ConnectedBundle（仅 connect() 调用路径）。
pub(super) async fn build_connected_bundle(request: ConnectRequest, mut raw: RawConnection) -> Result<ConnectedBundle, String> {
    let fs_provider = SshFsProvider::new(raw.sftp, tokio::runtime::Handle::current());
    let remote_projects_path = super::remote_path_resolver::resolve_remote_projects_path_static(
        &mut raw.session, &raw.merged_config.username, &fs_provider,
    ).await;
    let status = crate::types::ssh::SshConnectionStatus::connected(
        raw.merged_config.host.clone(),
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

/// Open SFTP subsystem with 8s timeout + diagnostic.
///
/// Signature (phase 2): (session) → (session, user, host). Private (same-file caller only).
async fn open_sftp_subsystem_static(
    session: &mut client::Handle<SshClientHandler>,
    user: &str,
    host: &str,
) -> Result<SftpSession, String> {
    let open_future = async {
        let channel = session.channel_open_session().await
            .map_err(|e| format!("Failed to open SSH session channel for SFTP: {}", e))?;
        channel.request_subsystem(true, "sftp").await
            .map_err(|e| format!("Failed to request SFTP subsystem: {}", e))?;
        let stream = channel.into_stream();
        SftpSession::new(stream).await
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
