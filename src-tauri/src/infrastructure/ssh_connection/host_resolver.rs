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
}

impl ResolvedHost {
    pub(super) fn fallback(host: &str) -> Self {
        Self {
            hostname: host.to_string(),
            port: 22,
            user: None,
            identity_files: Vec::new(),
            identity_agent: None,
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
    }
}

/// Resolve host via `ssh -G`. Fallback queries SshConfigParser by input_host.
///
/// `input_host` should be `request.original_host` (alias), NOT merged_config.host.
pub(super) async fn resolve_host(
    input_host: &str,
    config_parser: Option<&SshConfigParser>,
) -> ResolvedHost {
    let output = tokio::time::timeout(
        SSH_G_TIMEOUT,
        tokio::process::Command::new("ssh")
            .arg("-G")
            .arg(input_host)
            .kill_on_drop(true)
            .output(),
    )
    .await;

    let mut resolved = match output {
        Ok(Ok(o)) if o.status.success() => parse_ssh_g_output(&String::from_utf8_lossy(&o.stdout)),
        Ok(Ok(o)) => {
            log::warn!(
                "ssh -G exited non-zero {} for {}; falling back",
                o.status.code().unwrap_or(-1),
                input_host
            );
            ResolvedHost::fallback(input_host)
        }
        Ok(Err(e)) => {
            log::warn!("ssh -G spawn failed for {}: {}; falling back", input_host, e);
            ResolvedHost::fallback(input_host)
        }
        Err(_) => {
            log::warn!(
                "ssh -G timed out after {}s for {}; falling back",
                SSH_G_TIMEOUT.as_secs(),
                input_host
            );
            ResolvedHost::fallback(input_host)
        }
    };

    if resolved.identity_files.is_empty() {
        if let Some(parser) = config_parser {
            if let Some(entry) = parser.resolve_host(input_host) {
                if !entry.identity_files.is_empty() {
                    log::debug!(
                        "Backfilling {} identity_files from SshConfigParser for alias {}",
                        entry.identity_files.len(),
                        input_host
                    );
                    resolved.identity_files = entry
                        .identity_files
                        .iter()
                        .map(PathBuf::from)
                        .collect();
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
        let expected = home
            .join("Library/Group Containers/2BUA8C4S2C.com.1password/agent.sock");
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
        let output = "hostname x.example.com\nforwardagent yes\nuserknownhostsfile /dev/null\nuser dave\n";
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
}
