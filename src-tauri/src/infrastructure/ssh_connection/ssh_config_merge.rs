//! SSH 配置合并（静态化版本）。

use crate::infrastructure::ssh_config_parser::SshConfigParser;
use crate::types::ssh::{SshAuthMethod, SshConnectionConfig};

/// Merge a connection config with SSH config entries (static/free function version).
pub(super) fn merge_with_ssh_config_static(
    mut config: SshConnectionConfig,
    config_parser: Option<&SshConfigParser>,
) -> SshConnectionConfig {
    // 仅作 "fallback" — host/user 只填空字段；port 保留原逻辑（fallback 路径兜底）。
    // ssh -G（connect_flow 中调用 host_resolver）的输出在非 fallback 路径下覆盖这里的结果，
    // 与 Electron 优先级一致：ssh -G > ssh_config 文件。
    // 用户显式配置 > ssh -G（非 fallback）> ssh_config 文件 > ssh -G fallback (port=22) > 默认 22。
    //
    // 保留 port 块的原因：ssh -G fallback 时（Windows 无 OpenSSH 等）resolved.port=22 会
    // 覆盖 merged_config.port；ssh_config_merge 的 port 块在 fallback 之前先把 entry.port
    // 填进 merged_config.port，让 merge_resolved_into_config 能在 fallback 检测后保留它。
    if let Some(parser) = &config_parser {
        if let Some(entry) = parser.resolve_host(&config.host) {
            // host: 仅在用户未显式指定（空字符串）时使用 entry.host_name。
            // 通常 config.host 是 alias 不为空，entry.host_name 由 connect_flow 的 ssh -G
            // 合并接管（通过 merge_resolved_into_config）。
            if config.host.is_empty() {
                if let Some(ref host_name) = entry.host_name {
                    config.host = host_name.clone();
                }
            }
            if config.username.is_empty() {
                if let Some(ref user) = entry.user {
                    config.username = user.clone();
                }
            }
            // port: 保留原 fill-empty 逻辑。fallback 路径下（ssh -G 不可用），
            // merged_config.port 由此块设为 entry.port；merge_resolved_into_config
            // 通过 fallback 检测保留它。
            if config.port == 22 {
                if let Some(port) = entry.port {
                    config.port = port;
                }
            }
            if matches!(config.auth_method, SshAuthMethod::Auto) && !entry.identity_files.is_empty()
            {
                config.auth_method = SshAuthMethod::PrivateKey;
                if config.private_key_path.is_none() {
                    config.private_key_path = Some(entry.identity_files[0].clone());
                }
            }
        }
    }
    if config.username.is_empty() {
        config.username = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "root".to_string());
    }
    config
}
