//! SSH 连接管理器 — 管理 russh SSH 连接和 SFTP 会话。
//!
//! Phase 1: Connection lifecycle + event emission. Actual russh connection is stubbed.
//! TODO (full implementation): russh connect, auth, SFTP channel, remote exec.

use std::sync::Arc;

use tokio::sync::{RwLock, broadcast};

use crate::types::ssh::{
    SshAuthMethod, SshConfigHostEntry, SshConnectionConfig, SshConnectionStatus,
    SshTestResult,
};
#[cfg(test)]
use crate::types::ssh::SshConnectionState;
use crate::infrastructure::fs_provider::FsProvider;
use crate::infrastructure::ssh_config_parser::SshConfigParser;

/// Active SSH connection state (internal, not exposed to commands).
struct SshConnection {
    /// Merged config (user input + ssh_config resolution).
    config: SshConnectionConfig,
    /// Current connection status.
    status: SshConnectionStatus,
    /// Resolved remote projects path (e.g. `/home/user/.claude/projects`).
    remote_projects_path: Option<String>,
    // TODO: russh client handle, SFTP session
}

/// SSH connection manager — single-connection lifecycle model.
///
/// - `connect()` auto-disconnects any existing connection.
/// - `disconnect()` drops the current connection (no params).
/// - Status changes are broadcast via `tokio::sync::broadcast`.
/// - Phase 1: connection state machine only; russh connection is stubbed.
pub struct SshConnectionManager {
    /// Current active connection (single-connection model).
    connection: RwLock<Option<SshConnection>>,
    /// SSH config parser for host resolution.
    config_parser: Option<SshConfigParser>,
    /// Broadcast sender for status change events.
    event_sender: broadcast::Sender<SshConnectionStatus>,
}

impl SshConnectionManager {
    /// Create a new SSH connection manager.
    ///
    /// Initializes the broadcast channel (capacity 16) and attempts to
    /// load the default SSH config (`~/.ssh/config`). If the config file
    /// does not exist, the parser is `None` and config-based methods
    /// return empty results.
    pub fn new() -> Self {
        let (event_sender, _) = broadcast::channel::<SshConnectionStatus>(16);

        let config_parser = match SshConfigParser::from_default_path() {
            Ok(Some(parser)) => Some(parser),
            Ok(None) => None,
            Err(e) => {
                log::warn!("Failed to load SSH config: {}", e);
                None
            }
        };

        Self {
            connection: RwLock::new(None),
            config_parser,
            event_sender,
        }
    }

    /// Subscribe to connection status change events.
    ///
    /// Returns a `broadcast::Receiver` that the event bridge (Task 11)
    /// can poll to forward events to the Tauri frontend.
    pub fn subscribe(&self) -> broadcast::Receiver<SshConnectionStatus> {
        self.event_sender.subscribe()
    }

    /// Connect to an SSH server.
    ///
    /// Auto-disconnects any existing connection first (Electron-aligned behavior).
    /// Merges the provided config with `~/.ssh/config` if available.
    /// Phase 1: stubs the actual russh connection — immediately transitions to Connected.
    ///
    /// # Events emitted
    /// 1. `Connecting` (after auto-disconnect, if any)
    /// 2. `Connected` or `Error` (after connection attempt)
    pub async fn connect(
        &self,
        config: SshConnectionConfig,
    ) -> Result<SshConnectionStatus, String> {
        // Auto-disconnect any existing connection
        let _ = self.disconnect().await;

        // Emit connecting status
        let connecting_status = SshConnectionStatus::connecting(config.host.clone());
        let _ = self.event_sender.send(connecting_status.clone());

        // Merge with SSH config
        let merged_config = self.merge_with_ssh_config(config);

        // Validate required fields
        if merged_config.host.trim().is_empty() {
            let error_status =
                SshConnectionStatus::error(merged_config.host.clone(), "Host is required".into());
            let _ = self.event_sender.send(error_status.clone());
            return Ok(error_status);
        }
        if merged_config.username.trim().is_empty() {
            let error_status = SshConnectionStatus::error(
                merged_config.host.clone(),
                "Username is required".into(),
            );
            let _ = self.event_sender.send(error_status.clone());
            return Ok(error_status);
        }

        // TODO: Actual russh connection
        // Phase 1: stub — immediately "connected"
        let remote_projects_path = self.resolve_remote_home(&merged_config.username);
        let connected_status =
            SshConnectionStatus::connected(merged_config.host.clone(), remote_projects_path.clone());

        // Store the connection
        {
            let mut conn = self.connection.write().await;
            *conn = Some(SshConnection {
                config: merged_config,
                status: connected_status.clone(),
                remote_projects_path: Some(remote_projects_path),
            });
        }

        // Emit connected status
        let _ = self.event_sender.send(connected_status.clone());

        Ok(connected_status)
    }

    /// Disconnect the current SSH connection.
    ///
    /// Takes no parameters — disconnects whatever is active (Electron-aligned).
    /// No-op if already disconnected.
    ///
    /// # Events emitted
    /// - `Disconnected` (if there was an active connection)
    pub async fn disconnect(&self) -> Result<SshConnectionStatus, String> {
        let mut conn = self.connection.write().await;
        if conn.is_none() {
            return Ok(SshConnectionStatus::disconnected());
        }

        *conn = None;
        let status = SshConnectionStatus::disconnected();
        let _ = self.event_sender.send(status.clone());

        Ok(status)
    }

    /// Get the current active connection state.
    ///
    /// Returns `Disconnected` if no connection is active.
    pub async fn get_active_state(&self) -> SshConnectionStatus {
        let conn = self.connection.read().await;
        match conn.as_ref() {
            Some(c) => c.status.clone(),
            None => SshConnectionStatus::disconnected(),
        }
    }

    /// Test an SSH connection configuration.
    ///
    /// Validates host and username are non-empty. Does NOT create a real connection
    /// in Phase 1 — just validates the config fields.
    pub fn test(&self, config: &SshConnectionConfig) -> Result<SshTestResult, String> {
        if config.host.trim().is_empty() {
            return Ok(SshTestResult {
                success: false,
                error: Some("Host is required".into()),
            });
        }
        if config.username.trim().is_empty() {
            return Ok(SshTestResult {
                success: false,
                error: Some("Username is required".into()),
            });
        }

        // TODO: Phase 2 — actual connection test with timeout
        Ok(SshTestResult {
            success: true,
            error: None,
        })
    }

    /// Get all host entries from the SSH config.
    ///
    /// Returns an empty vec if no SSH config is loaded.
    pub fn get_config_hosts(&self) -> Vec<SshConfigHostEntry> {
        match &self.config_parser {
            Some(parser) => parser.get_hosts(),
            None => Vec::new(),
        }
    }

    /// Resolve a host alias from the SSH config.
    ///
    /// Returns `None` if the alias is not found or no SSH config is loaded.
    pub fn resolve_host_config(&self, alias: &str) -> Option<SshConfigHostEntry> {
        self.config_parser.as_ref()?.resolve_host(alias)
    }

    /// Get the remote projects path from the current connection.
    ///
    /// Returns `None` if not connected.
    pub async fn get_remote_projects_path(&self) -> Option<String> {
        let conn = self.connection.read().await;
        conn.as_ref()
            .and_then(|c| c.remote_projects_path.clone())
    }

    /// Get the FsProvider for the active SSH connection.
    ///
    /// Phase 1: always returns `None` (SFTP not implemented yet).
    /// Phase 2: returns `Some(Arc<dyn FsProvider>)` wrapping an SFTP provider.
    pub async fn get_provider(&self) -> Option<Arc<dyn FsProvider>> {
        // TODO: Phase 2 — return SFTP-based FsProvider when connected
        None
    }

    /// Discover the SSH agent socket path.
    ///
    /// Checks (in order):
    /// 1. `SSH_AUTH_SOCK` environment variable
    /// 2. macOS launchctl (`SSH_AUTH_SOCK` from launchctl getenv)
    /// 3. 1Password CLI agent socket paths
    /// 4. `~/.ssh/agent.sock`
    pub fn discover_agent_socket() -> Option<String> {
        // 1. Check SSH_AUTH_SOCK environment variable
        if let Ok(sock) = std::env::var("SSH_AUTH_SOCK") {
            if !sock.is_empty() {
                return Some(sock);
            }
        }

        // 2. macOS: check launchctl for SSH_AUTH_SOCK
        #[cfg(target_os = "macos")]
        {
            if let Ok(output) = std::process::Command::new("launchctl")
                .args(["getenv", "SSH_AUTH_SOCK"])
                .output()
            {
                let sock = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !sock.is_empty() {
                    return Some(sock);
                }
            }
        }

        // 3. 1Password CLI agent socket
        let home = dirs::home_dir()?;
        let op_sock = home.join(".1password").join("agent.sock");
        if op_sock.exists() {
            return Some(op_sock.to_string_lossy().to_string());
        }

        // 4. ~/.ssh/agent.sock
        let ssh_agent = home.join(".ssh").join("agent.sock");
        if ssh_agent.exists() {
            return Some(ssh_agent.to_string_lossy().to_string());
        }

        None
    }

    /// Merge a connection config with SSH config entries.
    ///
    /// If a host alias matches an entry in `~/.ssh/config`, fills in
    /// missing values (host_name -> host, user, port, identity_file -> private_key_path).
    fn merge_with_ssh_config(&self, config: SshConnectionConfig) -> SshConnectionConfig {
        let Some(parser) = &self.config_parser else {
            return config;
        };

        let Some(entry) = parser.resolve_host(&config.host) else {
            return config;
        };

        let mut merged = config;

        // Use HostName from config if host matches an alias and HostName is set
        if let Some(host_name) = entry.host_name {
            merged.host = host_name;
        }

        // Fill in username if not provided
        if merged.username.is_empty() {
            if let Some(user) = entry.user {
                merged.username = user;
            }
        }

        // Fill in port if not default
        if merged.port == 22 {
            if let Some(port) = entry.port {
                merged.port = port;
            }
        }

        // Set auth method to PrivateKey if IdentityFile is configured and method is Auto
        if matches!(merged.auth_method, SshAuthMethod::Auto) && entry.has_identity_file {
            merged.auth_method = SshAuthMethod::PrivateKey;
        }

        merged
    }

    /// Resolve the remote home directory for a user.
    ///
    /// Phase 1: stub that returns `/home/{username}/.claude/projects`.
    /// Phase 2: query the remote filesystem for the actual home directory.
    fn resolve_remote_home(&self, username: &str) -> String {
        // TODO: Phase 2 — query remote filesystem for actual home dir
        format!("/home/{}/.claude/projects", username)
    }
}

impl Default for SshConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> SshConnectionConfig {
        SshConnectionConfig {
            host: "example.com".into(),
            port: 22,
            username: "root".into(),
            auth_method: SshAuthMethod::Password,
            password: Some("secret".into()),
            private_key_path: None,
        }
    }

    #[test]
    fn test_connection_manager_new() {
        let manager = SshConnectionManager::new();
        // Should not panic; broadcast channel and optional config parser initialized
        assert!(manager.config_parser.is_some() || manager.config_parser.is_none());
    }

    #[test]
    fn test_default_impl() {
        let manager = SshConnectionManager::default();
        // Default should work the same as new()
        assert!(manager.event_sender.receiver_count() == 0);
    }

    #[tokio::test]
    async fn test_get_active_state_disconnected() {
        let manager = SshConnectionManager::new();
        let state = manager.get_active_state().await;
        assert!(matches!(state.state, SshConnectionState::Disconnected));
        assert!(state.host.is_none());
        assert!(state.error.is_none());
    }

    #[tokio::test]
    async fn test_connect_and_disconnect() {
        let manager = SshConnectionManager::new();

        // Connect
        let config = test_config();
        let status = manager.connect(config).await.unwrap();
        assert!(matches!(status.state, SshConnectionState::Connected));
        assert_eq!(status.host.as_deref(), Some("example.com"));

        // Verify active state
        let state = manager.get_active_state().await;
        assert!(matches!(state.state, SshConnectionState::Connected));

        // Disconnect
        let status = manager.disconnect().await.unwrap();
        assert!(matches!(status.state, SshConnectionState::Disconnected));

        // Verify back to disconnected
        let state = manager.get_active_state().await;
        assert!(matches!(state.state, SshConnectionState::Disconnected));
    }

    #[tokio::test]
    async fn test_connect_auto_disconnects_existing() {
        let manager = SshConnectionManager::new();

        // First connection
        let config1 = SshConnectionConfig {
            host: "first.com".into(),
            port: 22,
            username: "user1".into(),
            auth_method: SshAuthMethod::Password,
            password: None,
            private_key_path: None,
        };
        let status1 = manager.connect(config1).await.unwrap();
        assert_eq!(status1.host.as_deref(), Some("first.com"));

        // Second connection should auto-disconnect first
        let config2 = SshConnectionConfig {
            host: "second.com".into(),
            port: 22,
            username: "user2".into(),
            auth_method: SshAuthMethod::Agent,
            password: None,
            private_key_path: None,
        };
        let status2 = manager.connect(config2).await.unwrap();
        assert_eq!(status2.host.as_deref(), Some("second.com"));

        // Active state should reflect second connection
        let state = manager.get_active_state().await;
        assert_eq!(state.host.as_deref(), Some("second.com"));
    }

    #[tokio::test]
    async fn test_disconnect_when_not_connected() {
        let manager = SshConnectionManager::new();
        let status = manager.disconnect().await.unwrap();
        assert!(matches!(status.state, SshConnectionState::Disconnected));
    }

    #[tokio::test]
    async fn test_connect_validates_host() {
        let manager = SshConnectionManager::new();

        let config = SshConnectionConfig {
            host: "".into(),
            port: 22,
            username: "root".into(),
            auth_method: SshAuthMethod::Password,
            password: None,
            private_key_path: None,
        };
        let status = manager.connect(config).await.unwrap();
        assert!(matches!(status.state, SshConnectionState::Error));
        assert!(status.error.as_ref().unwrap().contains("Host"));
    }

    #[tokio::test]
    async fn test_connect_validates_username() {
        let manager = SshConnectionManager::new();

        let config = SshConnectionConfig {
            host: "example.com".into(),
            port: 22,
            username: "".into(),
            auth_method: SshAuthMethod::Password,
            password: None,
            private_key_path: None,
        };
        let status = manager.connect(config).await.unwrap();
        assert!(matches!(status.state, SshConnectionState::Error));
        assert!(status.error.as_ref().unwrap().contains("Username"));
    }

    #[tokio::test]
    async fn test_test_validation_success() {
        let manager = SshConnectionManager::new();
        let config = test_config();
        let result = manager.test(&config).unwrap();
        assert!(result.success);
        assert!(result.error.is_none());
    }

    #[tokio::test]
    async fn test_test_validation_empty_host() {
        let manager = SshConnectionManager::new();
        let config = SshConnectionConfig {
            host: "".into(),
            port: 22,
            username: "root".into(),
            auth_method: SshAuthMethod::Password,
            password: None,
            private_key_path: None,
        };
        let result = manager.test(&config).unwrap();
        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("Host"));
    }

    #[tokio::test]
    async fn test_test_validation_empty_username() {
        let manager = SshConnectionManager::new();
        let config = SshConnectionConfig {
            host: "example.com".into(),
            port: 22,
            username: "".into(),
            auth_method: SshAuthMethod::Password,
            password: None,
            private_key_path: None,
        };
        let result = manager.test(&config).unwrap();
        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("Username"));
    }

    #[tokio::test]
    async fn test_subscribe_receives_events() {
        let manager = SshConnectionManager::new();
        let mut rx = manager.subscribe();

        let config = test_config();
        let _ = manager.connect(config).await.unwrap();

        // Should receive at least "connecting" and "connected" events
        let mut received_states: Vec<SshConnectionState> = Vec::new();

        // Try to receive up to 2 events (non-blocking)
        while received_states.len() < 2 {
            match rx.try_recv() {
                Ok(status) => received_states.push(status.state),
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Lagged(n)) => {
                    log::debug!("Skipped {} lagged events", n);
                    break;
                }
                Err(broadcast::error::TryRecvError::Closed) => break,
            }
        }

        assert!(
            received_states.len() >= 2,
            "Expected at least 2 events, got {:?}",
            received_states.len()
        );
        assert!(
            received_states.iter().any(|s| matches!(s, SshConnectionState::Connecting)),
            "Expected Connecting event, got {:?}",
            received_states
        );
        assert!(
            received_states.iter().any(|s| matches!(s, SshConnectionState::Connected)),
            "Expected Connected event, got {:?}",
            received_states
        );
    }

    #[tokio::test]
    async fn test_get_remote_projects_path() {
        let manager = SshConnectionManager::new();

        // Not connected — None
        assert!(manager.get_remote_projects_path().await.is_none());

        // Connected — Some
        let config = test_config();
        let _ = manager.connect(config).await.unwrap();
        let path = manager.get_remote_projects_path().await;
        assert!(path.is_some());
        assert_eq!(path.unwrap(), "/home/root/.claude/projects");
    }

    #[tokio::test]
    async fn test_get_provider_phase1_none() {
        let manager = SshConnectionManager::new();
        assert!(manager.get_provider().await.is_none());
    }

    #[test]
    fn test_get_config_hosts() {
        let manager = SshConnectionManager::new();
        // Should not panic even if no SSH config exists
        let hosts = manager.get_config_hosts();
        // We can't assert specific entries since the test environment
        // may or may not have ~/.ssh/config
        // hosts is a Vec — just verify it doesn't panic
        let _ = &hosts;
    }

    #[test]
    fn test_resolve_host_config_not_found() {
        let manager = SshConnectionManager::new();
        let result = manager.resolve_host_config("nonexistent-host-xyz123");
        assert!(result.is_none());
    }

    #[test]
    fn test_merge_with_ssh_config_no_parser() {
        let manager = SshConnectionManager {
            connection: RwLock::const_new(None),
            config_parser: None,
            event_sender: broadcast::channel(1).0,
        };

        let config = test_config();
        let merged = manager.merge_with_ssh_config(config.clone());
        assert_eq!(merged.host, config.host);
        assert_eq!(merged.username, config.username);
        assert_eq!(merged.port, config.port);
    }

    #[test]
    fn test_resolve_remote_home() {
        let manager = SshConnectionManager::new();
        assert_eq!(manager.resolve_remote_home("admin"), "/home/admin/.claude/projects");
        assert_eq!(manager.resolve_remote_home("deploy"), "/home/deploy/.claude/projects");
    }

    #[test]
    fn test_discover_agent_socket() {
        // This test just verifies the function runs without panicking.
        // The result depends on the test environment.
        let _result = SshConnectionManager::discover_agent_socket();
    }
}
