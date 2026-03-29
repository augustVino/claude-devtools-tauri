//! SSH 类型定义 — 连接配置、状态、认证方式、事件等。

use serde::{Deserialize, Serialize};

/// SSH 连接配置 (前端传入)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshConnectionConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_method: SshAuthMethod,
    pub password: Option<String>,
    pub private_key_path: Option<String>,
}

/// SSH 认证方式。
/// CRITICAL: rename_all = "camelCase" 使 PrivateKey → "privateKey"
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SshAuthMethod {
    Password,
    PrivateKey,
    Agent,
    Auto,
}

/// SSH 连接状态 (命令返回)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshConnectionStatus {
    pub state: SshConnectionState,
    pub host: Option<String>,
    pub error: Option<String>,
    pub remote_projects_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SshConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

/// SSH Config Host 条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshConfigHostEntry {
    pub alias: String,
    pub host_name: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub has_identity_file: bool,
}

/// SSH 测试结果 — { success, error? }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshTestResult {
    pub success: bool,
    pub error: Option<String>,
}

/// 最后连接的 SSH 配置 (持久化)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshLastConnection {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_method: String,
    pub private_key_path: Option<String>,
}

/// SSH 内部事件（用于 Tauri 事件系统桥接）。
#[derive(Debug, Clone, Serialize)]
pub struct SshStatusChangedEvent {
    pub status: SshConnectionStatus,
}

impl SshConnectionStatus {
    pub fn disconnected() -> Self {
        Self {
            state: SshConnectionState::Disconnected,
            host: None,
            error: None,
            remote_projects_path: None,
        }
    }

    pub fn connecting(host: String) -> Self {
        Self {
            state: SshConnectionState::Connecting,
            host: Some(host),
            error: None,
            remote_projects_path: None,
        }
    }

    pub fn connected(host: String, remote_projects_path: String) -> Self {
        Self {
            state: SshConnectionState::Connected,
            host: Some(host),
            error: None,
            remote_projects_path: Some(remote_projects_path),
        }
    }

    pub fn error(host: String, error: String) -> Self {
        Self {
            state: SshConnectionState::Error,
            host: Some(host),
            error: Some(error),
            remote_projects_path: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_config_serializes_to_camel_case() {
        let config = SshConnectionConfig {
            host: "example.com".into(),
            port: 22,
            username: "root".into(),
            auth_method: SshAuthMethod::PrivateKey,
            password: None,
            private_key_path: Some("/home/user/.ssh/id_rsa".into()),
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("privateKeyPath"));
        assert!(json.contains("authMethod"));
        assert!(json.contains("privateKey"));
    }

    #[test]
    fn connection_state_serializes_to_lowercase() {
        let status = SshConnectionStatus::connected("host".into(), "/projects".into());
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"connected\""));
        assert!(json.contains("remoteProjectsPath"));
    }

    #[test]
    fn status_helper_methods() {
        let d = SshConnectionStatus::disconnected();
        assert!(matches!(d.state, SshConnectionState::Disconnected));
        assert!(d.host.is_none());

        let c = SshConnectionStatus::connecting("h".into());
        assert!(matches!(c.state, SshConnectionState::Connecting));
        assert_eq!(c.host.as_deref(), Some("h"));

        let ok = SshConnectionStatus::connected("h".into(), "/p".into());
        assert!(matches!(ok.state, SshConnectionState::Connected));
        assert_eq!(ok.remote_projects_path.as_deref(), Some("/p"));

        let e = SshConnectionStatus::error("h".into(), "fail".into());
        assert!(matches!(e.state, SshConnectionState::Error));
        assert_eq!(e.error.as_deref(), Some("fail"));
    }

    #[test]
    fn ssh_config_host_entry_serializes_to_camel_case() {
        let entry = SshConfigHostEntry {
            alias: "myhost".into(),
            host_name: Some("192.168.1.1".into()),
            user: Some("admin".into()),
            port: Some(2222),
            has_identity_file: true,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("hostName"));
        assert!(json.contains("hasIdentityFile"));
    }

    #[test]
    fn ssh_last_connection_serializes_to_camel_case() {
        let lc = SshLastConnection {
            host: "host".into(),
            port: 22,
            username: "user".into(),
            auth_method: "privateKey".into(),
            private_key_path: Some("/key".into()),
        };
        let json = serde_json::to_string(&lc).unwrap();
        assert!(json.contains("privateKeyPath"));
        assert!(json.contains("authMethod"));
    }

    #[test]
    fn roundtrip_connection_config() {
        let config = SshConnectionConfig {
            host: "example.com".into(),
            port: 22,
            username: "root".into(),
            auth_method: SshAuthMethod::Auto,
            password: Some("secret".into()),
            private_key_path: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: SshConnectionConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.host, config.host);
        assert_eq!(back.port, config.port);
        assert_eq!(back.username, config.username);
        assert!(matches!(back.auth_method, SshAuthMethod::Auto));
        assert_eq!(back.password, config.password);
        assert_eq!(back.private_key_path, config.private_key_path);
    }
}
