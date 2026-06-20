//! SSH host resolver — delegates to system `ssh -G` binary for full OpenSSH
//! config resolution (Match blocks, Include, IdentityAgent, etc.).
//!
//! Fallback when ssh binary missing/times out:
//! 1. Return input host as-is with default port 22
//! 2. Backfill identity_files from SshConfigParser (queried by input_host = alias)

use std::path::PathBuf;
use std::time::Duration;

use crate::infrastructure::ssh_auth::expand_tilde;
use crate::infrastructure::ssh_config_parser::SshConfigParser;

const SSH_G_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedHost {
    pub hostname: String,
    pub port: u16,
    pub user: Option<String>,
    pub identity_files: Vec<PathBuf>,
    pub identity_agent: Option<PathBuf>,
    /// true 表示 ssh -G 失败回退（hostname=input_host, port=22）。
    /// false 表示 ssh -G 真实解析成功。
    /// 用于 merge_resolved_into_config 判断是否跳过 port 覆盖（保护 ssh_config port）。
    pub was_fallback: bool,
}

impl ResolvedHost {
    pub(super) fn fallback(host: &str) -> Self {
        Self {
            hostname: host.to_string(),
            port: 22,
            user: None,
            identity_files: Vec::new(),
            identity_agent: None,
            was_fallback: true,
        }
    }
}

/// Parse `ssh -G` stdout into ResolvedHost (pure function for testing).
pub(super) fn parse_ssh_g_output(stdout: &str) -> ResolvedHost {
    let mut hostname: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut user: Option<String> = None;
    let mut identity_files: Vec<PathBuf> = Vec::new();
    let mut identity_agent: Option<PathBuf> = None;

    for raw_line in stdout.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = if let Some(idx) = line.find(char::is_whitespace) {
            (&line[..idx], line[idx..].trim_start())
        } else if let Some(idx) = line.find('=') {
            (&line[..idx], line[idx + 1..].trim())
        } else {
            continue;
        };
        let key_lower = key.to_lowercase();
        let value = value.to_string();

        match key_lower.as_str() {
            "hostname" => hostname = Some(value),
            "port" => {
                if let Ok(p) = value.parse::<u16>() {
                    port = Some(p);
                }
            }
            "user" => user = Some(value),
            "identityfile" => identity_files.push(expand_tilde(&value)),
            "identityagent" => identity_agent = Some(expand_tilde(&value)),
            _ => {}
        }
    }

    ResolvedHost {
        hostname: hostname.unwrap_or_default(),
        port: port.unwrap_or(22),
        user,
        identity_files,
        identity_agent,
        was_fallback: false,
    }
}

/// Default SSH key basenames that `ssh -G` emits when input doesn't match
/// any Host block. Aligned with Electron `looksLikeOnlyDefaultKeys`
/// (SshConnectionManager.ts:869-877, 7 names).
const DEFAULT_KEY_BASENAMES: &[&str] = &[
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "id_xmss",
    "id_ecdsa_sk",
    "id_ed25519_sk",
];

/// True iff every identity file is a default key under `~/.ssh/`.
/// Mirrors Electron's `looksLikeOnlyDefaultKeys`.
fn looks_like_only_default_keys(identity_files: &[PathBuf]) -> bool {
    if identity_files.is_empty() {
        return false;
    }
    let home_ssh = match dirs::home_dir() {
        Some(h) => h.join(".ssh"),
        None => return false,
    };
    identity_files.iter().all(|p| {
        p.parent().map(|d| d == home_ssh).unwrap_or(false)
            && p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| DEFAULT_KEY_BASENAMES.contains(&n))
                .unwrap_or(false)
    })
}

/// Find the first ssh_config alias whose HostName matches `hostname`.
/// Best-effort: returns None on missing parser or no match.
/// Mirrors Electron `findAliasByHostname` (SshConnectionManager.ts:442-452).
///
/// 注意：`SshConfigHostEntry.alias: String`（不是 Option），需要 `.clone()` 避免 move。
fn find_alias_by_hostname(
    parser: Option<&crate::infrastructure::ssh_config_parser::SshConfigParser>,
    hostname: &str,
) -> Option<String> {
    let parser = parser?;
    for entry in parser.get_hosts() {
        if entry.host_name.as_deref() == Some(hostname) {
            return Some(entry.alias.clone());
        }
    }
    None
}

/// Decide whether the ssh -G result carries useful inherited config.
/// Mirrors Electron `hasInheritedConfig` (SshConnectionManager.ts:415-420).
///
/// **已知 trade-off（用户决策方案 A，修正 codex 第四轮 H2）**：
/// `resolved.user.is_some()` 在 IP 直连场景会误判。实测 `ssh -G 1.2.3.4`
/// 总是输出 OS 用户名（如 `user vino`），即使 ssh_config 没有显式 User 指令。
/// 这导致 IP 直连场景 has_inherited_config 永远 true，**永不触发 alias 反查**。
///
/// **接受的限制**：
/// - Electron `Boolean(resolved.user)` 同样会误判，但 Electron 入口是 alias 不受影响
/// - Tauri Task 4 反查是 best-effort 增强，不触发不影响主流程（用户仍可手动配 IdentityFile）
/// - 与 Electron 一致的降级行为
///
/// 未来如需精确对齐"真实 ssh_config 配置"，可改为
/// `resolved.user.as_deref() != Some(&std::env::var("USER").unwrap_or_default())`
/// 但这引入跨平台 USER/USERNAME 差异和 Container $USER 未设等问题，本 plan 不做。
fn has_inherited_config(resolved: &ResolvedHost, input_host: &str) -> bool {
    (!resolved.identity_files.is_empty()
        && !looks_like_only_default_keys(&resolved.identity_files))
        || resolved.identity_agent.is_some()
        || resolved.user.is_some()
        || (!resolved.hostname.is_empty() && resolved.hostname != input_host)
}

/// Resolve host via `ssh -G`. Fallback queries SshConfigParser by input_host.
///
/// `input_host` should be `request.original_host` (alias), NOT merged_config.host.
pub(super) async fn resolve_host(
    input_host: &str,
    config_parser: Option<&SshConfigParser>,
) -> ResolvedHost {
    let first = run_ssh_g(input_host).await;
    if has_inherited_config(&first, input_host) {
        return apply_backfill(first, input_host, config_parser).await;
    }

    // No inherited config → try alias reverse-lookup by HostName.
    // Mirrors Electron SshConnectionManager.ts:422-430.
    if let Some(alias) = find_alias_by_hostname(config_parser, input_host) {
        log::debug!(
            "Reverse-resolved hostname {} to alias {}; re-running ssh -G",
            input_host,
            alias
        );
        let from_alias = run_ssh_g(&alias).await;
        if has_inherited_config(&from_alias, input_host) {
            return apply_backfill(from_alias, &alias, config_parser).await;
        }
    }

    apply_backfill(first, input_host, config_parser).await
}

/// Run `ssh -G <host>` and parse output. Returns fallback on any failure.
async fn run_ssh_g(host: &str) -> ResolvedHost {
    let output = tokio::time::timeout(
        SSH_G_TIMEOUT,
        tokio::process::Command::new("ssh")
            .arg("-G")
            .arg(host)
            .kill_on_drop(true)
            .output(),
    )
    .await;

    match output {
        Ok(Ok(o)) if o.status.success() => parse_ssh_g_output(&String::from_utf8_lossy(&o.stdout)),
        Ok(Ok(o)) => {
            log::warn!(
                "ssh -G exited non-zero {} for {}; falling back",
                o.status.code().unwrap_or(-1),
                host
            );
            ResolvedHost::fallback(host)
        }
        Ok(Err(e)) => {
            log::warn!("ssh -G spawn failed for {}: {}; falling back", host, e);
            ResolvedHost::fallback(host)
        }
        Err(_) => {
            log::warn!(
                "ssh -G timed out after {}s for {}; falling back",
                SSH_G_TIMEOUT.as_secs(),
                host
            );
            ResolvedHost::fallback(host)
        }
    }
}

/// Apply SshConfigParser identity_files backfill when ssh -G returned none.
async fn apply_backfill(
    mut resolved: ResolvedHost,
    lookup_key: &str,
    config_parser: Option<&SshConfigParser>,
) -> ResolvedHost {
    if resolved.identity_files.is_empty() {
        if let Some(parser) = config_parser {
            if let Some(entry) = parser.resolve_host(lookup_key) {
                if !entry.identity_files.is_empty() {
                    log::debug!(
                        "Backfilling {} identity_files from SshConfigParser for alias {}",
                        entry.identity_files.len(),
                        lookup_key
                    );
                    resolved.identity_files =
                        entry.identity_files.iter().map(PathBuf::from).collect();
                }
            }
        }
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic_output() {
        let output = "\
host example.com
    hostname example.com
    user alice
    port 2222
    identityfile ~/.ssh/id_ed25519
    identityfile ~/.ssh/id_rsa
";
        let resolved = parse_ssh_g_output(output);
        assert_eq!(resolved.hostname, "example.com");
        assert_eq!(resolved.port, 2222);
        assert_eq!(resolved.user.as_deref(), Some("alice"));
        assert_eq!(resolved.identity_files.len(), 2);
        let home = dirs::home_dir().unwrap();
        assert_eq!(resolved.identity_files[0], home.join(".ssh/id_ed25519"));
        assert_eq!(resolved.identity_files[1], home.join(".ssh/id_rsa"));
    }

    /// Real ssh -G output (no indent, has host line, multiple identityfile).
    /// Verifies parser handles actual production output format.
    #[test]
    fn test_parse_real_ssh_g_output_format() {
        let output = "\
user alice
hostname example.com
port 2222
addressfamily any
batchmode no
canonicalizehostname no
checkhostip no
compression no
connecttimeout 10
identityfile ~/.ssh/id_rsa
identityfile ~/.ssh/id_ecdsa
identityfile ~/.ssh/id_ecdsa_sk
identityfile ~/.ssh/id_ed25519
identityfile ~/.ssh/id_ed25519_sk
";
        let resolved = parse_ssh_g_output(output);
        assert_eq!(resolved.hostname, "example.com");
        assert_eq!(resolved.port, 2222);
        assert_eq!(resolved.user.as_deref(), Some("alice"));
        assert_eq!(resolved.identity_files.len(), 5);
        // Unknown keys (addressfamily, batchmode, etc.) silently ignored
    }

    #[test]
    fn test_parse_identityagent() {
        let output = "\
host 1password
    hostname corp.example.com
    user bob
    identityagent ~/Library/Group Containers/2BUA8C4S2C.com.1password/agent.sock
";
        let resolved = parse_ssh_g_output(output);
        let home = dirs::home_dir().unwrap();
        let expected = home.join("Library/Group Containers/2BUA8C4S2C.com.1password/agent.sock");
        assert_eq!(resolved.identity_agent.as_ref(), Some(&expected));
    }

    #[test]
    fn test_parse_equals_format() {
        let output = "hostname=example.com\nport=22\nuser=carol\n";
        let resolved = parse_ssh_g_output(output);
        assert_eq!(resolved.hostname, "example.com");
        assert_eq!(resolved.port, 22);
        assert_eq!(resolved.user.as_deref(), Some("carol"));
    }

    #[test]
    fn test_parse_empty_and_comments() {
        let output = "
# Comment
   # Indented comment

hostname only.example.com
";
        let resolved = parse_ssh_g_output(output);
        assert_eq!(resolved.hostname, "only.example.com");
        assert_eq!(resolved.port, 22);
        assert!(resolved.user.is_none());
        assert!(resolved.identity_files.is_empty());
    }

    #[test]
    fn test_parse_invalid_port_falls_back_to_22() {
        let resolved = parse_ssh_g_output("hostname x.example.com\nport not-a-number\n");
        assert_eq!(resolved.port, 22);
    }

    #[test]
    fn test_parse_port_out_of_range_falls_back_to_22() {
        // 99999 > u16::MAX
        let resolved = parse_ssh_g_output("hostname x.example.com\nport 99999\n");
        assert_eq!(resolved.port, 22);
    }

    #[test]
    fn test_parse_unknown_keys_ignored() {
        let output =
            "hostname x.example.com\nforwardagent yes\nuserknownhostsfile /dev/null\nuser dave\n";
        let resolved = parse_ssh_g_output(output);
        assert_eq!(resolved.hostname, "x.example.com");
        assert_eq!(resolved.user.as_deref(), Some("dave"));
    }

    #[test]
    fn test_parse_value_with_internal_spaces() {
        let resolved = parse_ssh_g_output("identityfile ~/My Documents/key");
        let home = dirs::home_dir().unwrap();
        assert_eq!(resolved.identity_files.len(), 1);
        assert_eq!(resolved.identity_files[0], home.join("My Documents/key"));
    }

    #[test]
    fn test_parse_crlf_line_endings() {
        let output = "hostname x.example.com\r\nuser dave\r\nport 2222\r\n";
        let resolved = parse_ssh_g_output(output);
        assert_eq!(resolved.hostname, "x.example.com");
        assert_eq!(resolved.user.as_deref(), Some("dave"));
        assert_eq!(resolved.port, 2222);
    }

    #[test]
    fn test_parse_ipv6_hostname() {
        let resolved = parse_ssh_g_output("hostname 2001:db8::1\nport 22\nuser carol\n");
        assert_eq!(resolved.hostname, "2001:db8::1");
        assert_eq!(resolved.port, 22);
    }

    #[test]
    fn test_fallback_returns_input_host() {
        let resolved = ResolvedHost::fallback("my-alias");
        assert_eq!(resolved.hostname, "my-alias");
        assert_eq!(resolved.port, 22);
        assert!(resolved.user.is_none());
        assert!(resolved.identity_files.is_empty());
        assert!(resolved.identity_agent.is_none());
    }

    #[test]
    fn test_looks_like_only_default_keys_empty_returns_false() {
        assert!(!looks_like_only_default_keys(&[]));
    }

    #[test]
    fn test_looks_like_only_default_keys_all_defaults() {
        // 用 unwrap_or 兼容容器化 CI（Alpine 无 HOME）
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        let ssh = home.join(".ssh");
        let files = vec![
            ssh.join("id_ed25519"),
            ssh.join("id_rsa"),
            ssh.join("id_ecdsa_sk"),
        ];
        assert!(looks_like_only_default_keys(&files));
    }

    #[test]
    fn test_looks_like_only_default_keys_has_custom_key() {
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        let ssh = home.join(".ssh");
        let files = vec![ssh.join("id_ed25519"), ssh.join("custom_company_key")];
        assert!(!looks_like_only_default_keys(&files));
    }

    #[test]
    fn test_looks_like_only_default_keys_rejects_non_ssh_dir() {
        // 同名文件但在其他目录 → 不算默认 key
        let files = vec![std::path::PathBuf::from("/tmp/.ssh/id_rsa")];
        assert!(!looks_like_only_default_keys(&files));
    }

    #[test]
    fn test_has_inherited_config_default_keys_only_is_false() {
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        let resolved = ResolvedHost {
            hostname: "10.0.0.1".to_string(),
            port: 22,
            user: None,
            identity_files: vec![home.join(".ssh/id_rsa")],
            identity_agent: None,
            was_fallback: false,
        };
        // 默认 key + hostname 与 input 相同 + 无 user/agent → false
        assert!(!has_inherited_config(&resolved, "10.0.0.1"));
    }

    #[test]
    fn test_has_inherited_config_custom_key_is_true() {
        let resolved = ResolvedHost {
            hostname: "10.0.0.1".to_string(),
            port: 22,
            user: None,
            identity_files: vec![std::path::PathBuf::from("/home/u/.ssh/company_key")],
            identity_agent: None,
            was_fallback: false,
        };
        assert!(has_inherited_config(&resolved, "10.0.0.1"));
    }

    #[test]
    fn test_has_inherited_config_identity_agent_is_true() {
        let resolved = ResolvedHost {
            hostname: "x".to_string(),
            port: 22,
            user: None,
            identity_files: vec![],
            identity_agent: Some(std::path::PathBuf::from("/tmp/agent.sock")),
            was_fallback: false,
        };
        assert!(has_inherited_config(&resolved, "x"));
    }

    #[test]
    fn test_has_inherited_config_explicit_user_is_true() {
        let resolved = ResolvedHost {
            hostname: "x".to_string(),
            port: 22,
            user: Some("deploy".to_string()),
            identity_files: vec![],
            identity_agent: None,
            was_fallback: false,
        };
        assert!(has_inherited_config(&resolved, "x"));
    }

    #[test]
    fn test_has_inherited_config_hostname_differs_from_input_is_true() {
        let resolved = ResolvedHost {
            hostname: "real.corp.com".to_string(),
            port: 22,
            user: None,
            identity_files: vec![],
            identity_agent: None,
            was_fallback: false,
        };
        assert!(has_inherited_config(&resolved, "alias"));
    }

    #[test]
    fn test_find_alias_by_hostname_match() {
        use crate::infrastructure::ssh_config_parser::SshConfigParser;
        let cfg = "Host myserver\n    HostName 10.0.0.5\n    User deploy\n";
        let parser = SshConfigParser::from_str(cfg).unwrap();
        let alias = find_alias_by_hostname(Some(&parser), "10.0.0.5");
        assert_eq!(alias.as_deref(), Some("myserver"));
    }

    #[test]
    fn test_find_alias_by_hostname_no_match() {
        use crate::infrastructure::ssh_config_parser::SshConfigParser;
        let cfg = "Host myserver\n    HostName 10.0.0.5\n";
        let parser = SshConfigParser::from_str(cfg).unwrap();
        assert!(find_alias_by_hostname(Some(&parser), "10.0.0.99").is_none());
    }

    #[test]
    fn test_find_alias_by_hostname_none_parser() {
        assert!(find_alias_by_hostname(None, "anything").is_none());
    }
}
