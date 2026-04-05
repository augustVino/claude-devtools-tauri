//! test() 方法 — 复用 establish_raw_connection 验证连接可用性。

use crate::types::ssh::{SshConnectionConfig, SshTestResult};

use super::ConnectRequest;

impl super::SshConnectionManager {
    /// Test an SSH connection configuration.
    ///
    /// Creates a temporary SSH session, authenticates, opens SFTP to verify
    /// full access, then disconnects. Returns success/failure without
    /// affecting the manager's active connection state.
    ///
    /// 复用 establish_raw_connection() 消除与 connect() 的重复代码。
    pub async fn test(&self, config: &SshConnectionConfig) -> Result<SshTestResult, String> {
        if config.host.trim().is_empty() {
            return Ok(SshTestResult { success: false, error: Some("Host is required".into()) });
        }

        let request = ConnectRequest::new(config.clone());

        // 复用核心连接逻辑（与 connect 完全相同的 Steps 4-10）
        match super::connect_flow::establish_raw_connection(&request, self.config_parser.as_ref()).await {
            Ok(raw) => {
                // test 不需要 FsProvider/远程路径/存储 → 直接断开
                let _ = raw.session.disconnect(russh::Disconnect::ByApplication, "", "").await;
                // raw.sftp drop → SFTP session 自动关闭
                Ok(SshTestResult { success: true, error: None })
            }
            Err(e) => Ok(SshTestResult { success: false, error: Some(e) }),
        }
    }
}
