//! 内部连接状态结构体。

use crate::infrastructure::ssh_fs_provider::SshFsProvider;
use crate::types::ssh::{SshConnectionConfig, SshConnectionStatus};
use russh::client;
use tokio::sync::watch;

/// Active SSH connection state (internal, not exposed to commands).
pub(crate) struct SshConnection {
    /// Merged config (user input + ssh_config resolution).
    #[allow(dead_code)]
    pub config: SshConnectionConfig,
    /// Current connection status.
    pub status: SshConnectionStatus,
    /// Resolved remote projects path.
    #[allow(dead_code)]
    pub remote_projects_path: Option<String>,
    /// Active russh SSH session handle.
    pub session: client::Handle<super::SshClientHandler>,
    /// SFTP-backed filesystem provider.
    pub fs_provider: SshFsProvider,
    /// Watch channel sender to signal the monitor to stop.
    pub monitor_stop: watch::Sender<bool>,
}
