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
    // Phase 1: Merge + Validate
    let merged_config = super::ssh_config_merge::merge_with_ssh_config_static(request.config.clone(), config_parser);
    if merged_config.host.trim().is_empty() { return Err("Host is required".into()); }

    // Phase 2: TCP Connect (10s timeout)
    let addr = (merged_config.host.as_str(), merged_config.port);
    let russh_config = Arc::new(russh::client::Config::default());
    let session = match tokio::time::timeout(CONNECT_TIMEOUT, russh::client::connect(russh_config, addr, SshClientHandler)).await {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => return Err(format!("SSH connection failed: {}", e)),
        Err(_) => return Err(format!("timed out after {}s", CONNECT_TIMEOUT.as_secs())),
    };

    // Phase 3: Discover agent socket
    let agent_socket = super::agent_discovery::discover_agent_socket_static();

    // Phase 4: Authenticate（使用 request.original_host 计算 resolved_alias）
    let mut session_mut = session;
    let resolved_alias = if request.original_host != merged_config.host {
        Some(request.original_host.clone())
    } else { None };

    if let Err(e) = ssh_auth::authenticate(
        &mut session_mut, &merged_config.username, &merged_config.auth_method,
        merged_config.password.as_deref(), merged_config.private_key_path.as_deref(),
        config_parser, resolved_alias.as_deref(), agent_socket.as_deref(),
    ).await { return Err(format!("authentication failed: {}", e)); }

    // Phase 5: Open SFTP
    let sftp = open_sftp_subsystem_static(&mut session_mut).await?;

    Ok(RawConnection { merged_config, original_host: request.original_host.clone(), session: session_mut, sftp })
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

/// 打开 SFTP 子系统（自由函数版本）。
async fn open_sftp_subsystem_static(session: &mut client::Handle<SshClientHandler>) -> Result<SftpSession, String> {
    let channel = session.channel_open_session().await
        .map_err(|e| format!("Failed to open SSH session channel for SFTP: {}", e))?;
    channel.request_subsystem(true, "sftp").await
        .map_err(|e| format!("Failed to request SFTP subsystem: {}", e))?;
    let stream = channel.into_stream();
    SftpSession::new(stream).await
        .map_err(|e| format!("Failed to initialize SFTP session: {}", e))
}
