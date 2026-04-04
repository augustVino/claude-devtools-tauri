//! SSH 连接流程中间态类型定义。
//!
//! 三层设计：
//!   ConnectRequest（用户输入 + original_host 快照）
//!     → RawConnection（TCP + Auth + SFTP 裸资源）
//!       → ConnectedBundle（业务层：FsProvider + 路径 + 状态）

use russh::client;
use russh_sftp::client::SftpSession;

use crate::infrastructure::ssh_fs_provider::SshFsProvider;
use crate::types::ssh::{SshConnectionConfig, SshConnectionStatus};

/// 原始连接请求上下文 — 在 SSH config merge 之前捕获原始 host。
///
/// `original_host` 是用户输入的 host（可能是 ssh_config alias），
/// 用于认证阶段计算 `resolved_alias`（当 HostName 覆盖了 alias 时）。
pub struct ConnectRequest {
    /// 用户原始输入的 config（未 merge）。
    pub config: SshConnectionConfig,
    /// 原始 host 快照（等于 config.host，在 merge 前保存）。
    pub original_host: String,
}

impl ConnectRequest {
    pub fn new(config: SshConnectionConfig) -> Self {
        let original_host = config.host.clone();
        Self { config, original_host }
    }
}

/// 原始连接结果 — TCP + 认证 + SFTP 全部完成后的裸资源。
///
/// 这是 connect() 和 test() 的共享中间产物。
/// 不包含 FsProvider、remote_path、status 等"上层语义"。
pub struct RawConnection {
    /// Merge 后的完整配置。
    pub merged_config: SshConnectionConfig,
    /// 原始 host alias（已用于 resolved_alias 计算，保留供调试）。
    pub original_host: String,
    /// 已认证的 russh session handle（所有权转移至此）。
    pub session: client::Handle<super::SshClientHandler>,
    /// 已打开的 SFTP session（所有权转移至此）。
    pub sftp: SftpSession,
}

/// 完整连接产物 — RawConnection + 业务层包装（仅 connect() 使用）。
pub struct ConnectedBundle {
    /// 原始请求上下文。
    pub request: ConnectRequest,
    /// Merge 后的完整配置。
    pub merged_config: SshConnectionConfig,
    /// 原始 host alias（已用于 resolved_alias 计算）。
    pub original_host: String,
    /// 已认证的 russh session handle（所有权转移至此）。
    pub session: russh::client::Handle<super::SshClientHandler>,
    /// SFTP 文件系统提供者。
    pub fs_provider: SshFsProvider,
    /// 解析出的远程项目根路径。
    pub remote_projects_path: String,
    /// 构建好的连接状态（用于事件发射和返回值）。
    pub status: SshConnectionStatus,
}
