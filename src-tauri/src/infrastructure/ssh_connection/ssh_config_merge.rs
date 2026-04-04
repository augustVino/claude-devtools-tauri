//! SSH 配置合并（静态化版本）。

use crate::infrastructure::ssh_config_parser::SshConfigParser;
use crate::types::ssh::{SshAuthMethod, SshConnectionConfig};

/// Merge a connection config with SSH config entries (static/free function version).
pub(super) fn merge_with_ssh_config_static(
    mut config: SshConnectionConfig,
    config_parser: Option<&SshConfigParser>,
) -> SshConnectionConfig {
    if let Some(parser) = &config_parser {
        if let Some(entry) = parser.resolve_host(&config.host) {
            if let Some(ref host_name) = entry.host_name { config.host = host_name.clone(); }
            if config.username.is_empty() {
                if let Some(ref user) = entry.user { config.username = user.clone(); }
            }
            if config.port == 22 { if let Some(port) = entry.port { config.port = port; } }
            if matches!(config.auth_method, SshAuthMethod::Auto) && entry.has_identity_file {
                config.auth_method = SshAuthMethod::PrivateKey;
            }
        }
    }
    if config.username.is_empty() {
        config.username = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_else(|_| "root".to_string());
    }
    config
}
