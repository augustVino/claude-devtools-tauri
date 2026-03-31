//! SSH config parser — reads `~/.ssh/config` and extracts host entries.
//!
//! Uses the `ssh_config` crate (v0.1.0) for config value resolution.
//! Since the crate's `SSHConfig.entries` field is private (no public iterator),
//! we extract host patterns from the raw text with a lightweight line parser,
//! then use `SSHConfig::query()` to resolve settings per host.
//!
//! All resolved data is owned (no borrowed lifetimes) so the struct is
//! `Send + Sync` without unsafe code.

use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use ssh_config::SSHConfig;

use crate::types::ssh::SshConfigHostEntry;

/// Default SSH config path: `~/.ssh/config`.
fn default_ssh_config_path() -> PathBuf {
    dirs::home_dir()
        .expect("HOME directory not found")
        .join(".ssh")
        .join("config")
}

/// Check if a host pattern contains wildcards (`*` or `?`) or starts with `!`.
fn is_wildcard_pattern(pattern: &str) -> bool {
    pattern.contains('*')
        || pattern.contains('?')
        || pattern.starts_with('!')
}

/// Extract unique non-wildcard host aliases from raw SSH config text.
///
/// Scans for `Host` directives and splits comma-separated patterns,
/// filtering out wildcards and negated entries.
fn extract_host_aliases(config_text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut aliases = Vec::new();

    for line in config_text.lines() {
        let trimmed = line.trim();
        // Skip comments and empty lines
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Look for "Host" keyword at start of line (case-sensitive, as per ssh_config spec)
        if let Some(rest) = trimmed.strip_prefix("Host ") {
            // Split comma-separated patterns
            for pattern in rest.split(',') {
                let pattern = pattern.trim();
                if pattern.is_empty() || is_wildcard_pattern(pattern) {
                    continue;
                }
                if seen.insert(pattern.to_string()) {
                    aliases.push(pattern.to_string());
                }
            }
        }
    }

    aliases
}

/// Resolve a host alias into an `SshConfigHostEntry` using the parsed config.
///
/// Returns fully owned data (no borrowed lifetimes).
fn resolve_entry(alias: &str, config: &SSHConfig) -> SshConfigHostEntry {
    let settings = config.query(alias);

    // HostName: only return if different from alias (Electron behavior)
    let host_name = settings
        .get("HostName")
        .map(|v| v.to_string())
        .filter(|hn| hn != alias);

    // Port: only return if != 22 (Electron behavior)
    let port = settings
        .get("Port")
        .and_then(|v| v.parse::<u16>().ok())
        .filter(|&p| p != 22);

    // User
    let user = settings.get("User").map(|v| v.to_string());

    // IdentityFile: check if explicitly configured
    let has_identity_file = settings.contains_key("IdentityFile");

    SshConfigHostEntry {
        alias: alias.to_string(),
        host_name,
        user,
        port,
        has_identity_file,
    }
}

/// Expand `Include` directives in SSH config content.
///
/// Processes `Include` and `include` directives with:
/// - Tilde expansion (`~` → home dir)
/// - Glob pattern matching (`*`, `?`)
/// - Silent skip on unreadable/missing files
/// - Recursive inclusion (included files may themselves have Include directives)
/// - Symlink loop detection via canonical path tracking
fn expand_includes(content: &str, max_depth: usize) -> String {
    expand_includes_inner(content, max_depth, &mut HashSet::new())
}

fn expand_includes_inner(
    content: &str,
    max_depth: usize,
    visited: &mut HashSet<PathBuf>,
) -> String {
    if max_depth == 0 {
        return content.to_string();
    }

    let home = dirs::home_dir().unwrap_or_default();
    let mut result = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // Match "Include" or "include" directive
        let pattern = if let Some(rest) = trimmed
            .strip_prefix("Include ")
            .or_else(|| trimmed.strip_prefix("include "))
        {
            rest.trim()
        } else {
            result.push(line.to_string());
            continue;
        };

        // Tilde expansion
        let expanded = if pattern.starts_with("~/") {
            home.join(&pattern[2..])
                .to_string_lossy()
            .to_string()
        } else {
            pattern.to_string()
        };

        // Check if pattern contains glob characters
        if expanded.contains('*') || expanded.contains('?') {
            // Glob expansion
            let dir = Path::new(&expanded)
                .parent()
                .unwrap_or(Path::new("."));
            let glob_part = Path::new(&expanded)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default();

            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if glob_matches(&glob_part, &name) {
                        let file_path = entry.path();
                        // Canonicalize for symlink loop detection; skip on failure
                        let canonical = match file_path.canonicalize() {
                            Ok(c) => c,
                            Err(_) => continue,
                        };
                        if visited.contains(&canonical) {
                            continue;
                        }
                        visited.insert(canonical);
                        if let Ok(included) = fs::read_to_string(&file_path) {
                            let expanded_content =
                                expand_includes_inner(&included, max_depth - 1, visited);
                            result.push(expanded_content);
                        }
                    }
                }
            }
        } else {
            // Single file include
            let file_path = Path::new(&expanded);
            // Canonicalize for symlink loop detection; skip on failure
            let canonical = match file_path.canonicalize() {
                Ok(c) => c,
                Err(_) => {
                    // File doesn't exist — silent skip (matching Electron behavior)
                    result.push(line.to_string());
                    continue;
                }
            };
            if visited.contains(&canonical) {
                continue;
            }
            visited.insert(canonical);
            if let Ok(included) = fs::read_to_string(file_path) {
                let expanded_content = expand_includes_inner(&included, max_depth - 1, visited);
                result.push(expanded_content);
            }
            // Silent skip on unreadable files (matching Electron behavior)
        }
    }

    result.join("\n")
}

/// Simple glob matching supporting `*` and `?` patterns.
fn glob_matches(pattern: &str, text: &str) -> bool {
    let p_chars: Vec<char> = pattern.chars().collect();
    let t_chars: Vec<char> = text.chars().collect();
    glob_match_inner(&p_chars, &t_chars, 0, 0)
}

fn glob_match_inner(pattern: &[char], text: &[char], pi: usize, ti: usize) -> bool {
    if pi == pattern.len() {
        return ti == text.len();
    }

    match pattern[pi] {
        '*' => {
            // Try matching * with 0..N characters
            for i in ti..=text.len() {
                if glob_match_inner(pattern, text, pi + 1, i) {
                    return true;
                }
            }
            false
        }
        '?' => {
            if ti < text.len() {
                glob_match_inner(pattern, text, pi + 1, ti + 1)
            } else {
                false
            }
        }
        c => {
            if ti < text.len() && text[ti] == c {
                glob_match_inner(pattern, text, pi + 1, ti + 1)
            } else {
                false
            }
        }
    }
}

/// Parse the SSH config text and resolve all host entries into owned data.
///
/// This function owns the `SSHConfig` and all borrowed data within a single
/// scope, producing fully owned output — no lifetime gymnastics needed.
fn parse_and_resolve(config_text: &str) -> Result<Vec<SshConfigHostEntry>, String> {
    let config = SSHConfig::parse_str(config_text)
        .map_err(|e| format!("Failed to parse SSH config: {:?}", e))?;

    let aliases = extract_host_aliases(config_text);
    let entries: Vec<SshConfigHostEntry> = aliases
        .iter()
        .map(|alias| resolve_entry(alias, &config))
        .collect();

    Ok(entries)
}

/// SSH config parser.
///
/// Parses `~/.ssh/config` (or a custom path) and provides methods to:
/// - List all non-wildcard host entries
/// - Resolve a specific host alias to its full configuration
///
/// All data is fully owned — this struct is `Send + Sync` without unsafe code.
pub struct SshConfigParser {
    /// Resolved host entries keyed by alias for O(1) lookup.
    entries_by_alias: HashMap<String, SshConfigHostEntry>,
    /// Host aliases in file order.
    aliases: Vec<String>,
}

impl SshConfigParser {
    /// Parse the default SSH config file (`~/.ssh/config`).
    ///
    /// Returns `Ok(None)` if the file does not exist.
    /// Returns `Err` if the file exists but cannot be read or parsed.
    pub fn from_default_path() -> Result<Option<Self>, String> {
        Self::from_path(&default_ssh_config_path())
    }

    /// Parse a specific SSH config file.
    ///
    /// Returns `Ok(None)` if the file does not exist.
    /// Returns `Err` if the file exists but cannot be read or parsed.
    pub fn from_path(path: &Path) -> Result<Option<Self>, String> {
        if !path.exists() {
            return Ok(None);
        }

        let config_text =
            fs::read_to_string(path).map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;

        // Expand Include directives before parsing
        let expanded_text = expand_includes(&config_text, 10);
        Self::from_str(&expanded_text).map(Some)
    }

    /// Parse SSH config from a string (useful for testing).
    pub fn from_str(config_text: &str) -> Result<Self, String> {
        let entries = parse_and_resolve(config_text)?;
        let aliases: Vec<String> = entries.iter().map(|e| e.alias.clone()).collect();
        let entries_by_alias: HashMap<String, SshConfigHostEntry> = entries
            .into_iter()
            .map(|e| (e.alias.clone(), e))
            .collect();

        Ok(Self {
            entries_by_alias,
            aliases,
        })
    }

    /// Get all non-wildcard host entries from the config (in file order).
    pub fn get_hosts(&self) -> Vec<SshConfigHostEntry> {
        self.aliases
            .iter()
            .filter_map(|alias| self.entries_by_alias.get(alias).cloned())
            .collect()
    }

    /// Resolve a specific host alias to its configuration.
    ///
    /// Returns `None` if the alias is not found in the config.
    pub fn resolve_host(&self, alias: &str) -> Option<SshConfigHostEntry> {
        self.entries_by_alias.get(alias).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_config(content: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("Failed to create temp file");
        write!(file, "{}", content).expect("Failed to write temp file");
        file
    }

    #[test]
    fn test_parse_empty_config() {
        let file = write_config("");
        let parser = SshConfigParser::from_path(file.path()).unwrap().unwrap();
        assert!(parser.get_hosts().is_empty());
    }

    #[test]
    fn test_parse_config_no_hosts() {
        let file = write_config("# Just a comment\nHost *\n  ForwardAgent yes\n");
        let parser = SshConfigParser::from_path(file.path()).unwrap().unwrap();
        assert!(parser.get_hosts().is_empty());
    }

    #[test]
    fn test_parse_host_entry() {
        let file = write_config(
            r#"Host myserver
    HostName 192.168.1.100
    User admin
    Port 2222
    IdentityFile ~/.ssh/id_rsa
"#,
        );
        let parser = SshConfigParser::from_path(file.path()).unwrap().unwrap();
        let hosts = parser.get_hosts();
        assert_eq!(hosts.len(), 1);

        let host = &hosts[0];
        assert_eq!(host.alias, "myserver");
        assert_eq!(host.host_name.as_deref(), Some("192.168.1.100"));
        assert_eq!(host.user.as_deref(), Some("admin"));
        assert_eq!(host.port, Some(2222));
        assert!(host.has_identity_file);
    }

    #[test]
    fn test_skip_wildcard_hosts() {
        let file = write_config(
            r#"Host *.example.com
    User wildcard

Host !exclude
    User excluded

Host *
    User catchall

Host realserver
    User real
"#,
        );
        let parser = SshConfigParser::from_path(file.path()).unwrap().unwrap();
        let hosts = parser.get_hosts();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "realserver");
        assert_eq!(hosts[0].user.as_deref(), Some("real"));
    }

    #[test]
    fn test_resolve_host() {
        let file = write_config(
            r#"Host web1
    HostName web.example.com
    User deploy

Host db1
    HostName db.example.com
    User postgres
"#,
        );
        let parser = SshConfigParser::from_path(file.path()).unwrap().unwrap();

        // Resolve existing host
        let web1 = parser.resolve_host("web1").unwrap();
        assert_eq!(web1.alias, "web1");
        assert_eq!(web1.host_name.as_deref(), Some("web.example.com"));

        // Resolve non-existing host
        assert!(parser.resolve_host("nonexistent").is_none());
    }

    #[test]
    fn test_port_normalization() {
        // Port 22 (default) should not be returned
        let file = write_config(
            r#"Host default-port
    HostName example.com
    Port 22

Host custom-port
    HostName example.com
    Port 2222

Host no-port
    HostName example.com
"#,
        );
        let parser = SshConfigParser::from_path(file.path()).unwrap().unwrap();
        let hosts = parser.get_hosts();

        let default_port = hosts.iter().find(|h| h.alias == "default-port").unwrap();
        assert_eq!(default_port.port, None, "Port 22 should not be returned");

        let custom_port = hosts.iter().find(|h| h.alias == "custom-port").unwrap();
        assert_eq!(custom_port.port, Some(2222));

        let no_port = hosts.iter().find(|h| h.alias == "no-port").unwrap();
        assert_eq!(no_port.port, None);
    }

    #[test]
    fn test_hostname_normalization() {
        // HostName same as alias should not be returned
        let file = write_config(
            r#"Host same-as-alias
    HostName same-as-alias

Host different-hostname
    HostName 192.168.1.1

Host no-hostname
    User admin
"#,
        );
        let parser = SshConfigParser::from_path(file.path()).unwrap().unwrap();
        let hosts = parser.get_hosts();

        let same = hosts.iter().find(|h| h.alias == "same-as-alias").unwrap();
        assert_eq!(
            same.host_name, None,
            "HostName same as alias should not be returned"
        );

        let different = hosts.iter().find(|h| h.alias == "different-hostname").unwrap();
        assert_eq!(different.host_name.as_deref(), Some("192.168.1.1"));

        let no_hn = hosts.iter().find(|h| h.alias == "no-hostname").unwrap();
        assert_eq!(no_hn.host_name, None);
    }

    #[test]
    fn test_has_identity_file() {
        let file = write_config(
            r#"Host with-key
    IdentityFile ~/.ssh/id_ed25519

Host without-key
    HostName example.com
"#,
        );
        let parser = SshConfigParser::from_path(file.path()).unwrap().unwrap();
        let hosts = parser.get_hosts();

        let with_key = hosts.iter().find(|h| h.alias == "with-key").unwrap();
        assert!(with_key.has_identity_file);

        let without_key = hosts.iter().find(|h| h.alias == "without-key").unwrap();
        assert!(!without_key.has_identity_file);
    }

    #[test]
    fn test_multiple_hosts() {
        let file = write_config(
            r#"Host server1
    HostName 10.0.0.1
    User root

Host server2
    HostName 10.0.0.2
    User deploy
    Port 8022

Host server3
    User guest
"#,
        );
        let parser = SshConfigParser::from_path(file.path()).unwrap().unwrap();
        let hosts = parser.get_hosts();
        assert_eq!(hosts.len(), 3);

        // Verify ordering matches file order
        assert_eq!(hosts[0].alias, "server1");
        assert_eq!(hosts[1].alias, "server2");
        assert_eq!(hosts[2].alias, "server3");

        assert_eq!(hosts[1].port, Some(8022));
        assert_eq!(hosts[2].host_name, None);
    }

    #[test]
    fn test_from_str() {
        let config_text = r#"Host test
    HostName test.example.com
    User tester
"#;
        let parser = SshConfigParser::from_str(config_text).unwrap();
        let hosts = parser.get_hosts();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "test");
    }

    #[test]
    fn test_comma_separated_hosts() {
        // Comma-separated host patterns should each be extracted as separate aliases
        let file = write_config(
            r#"Host alpha,beta,gamma
    HostName shared.example.com
    User shared
"#,
        );
        let parser = SshConfigParser::from_path(file.path()).unwrap().unwrap();
        let hosts = parser.get_hosts();
        assert_eq!(hosts.len(), 3);

        let aliases: Vec<&str> = hosts.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(aliases, vec!["alpha", "beta", "gamma"]);

        // All should resolve to the same settings
        for host in &hosts {
            assert_eq!(host.host_name.as_deref(), Some("shared.example.com"));
            assert_eq!(host.user.as_deref(), Some("shared"));
        }
    }

    #[test]
    fn test_comments_and_empty_lines() {
        let file = write_config(
            r#"# This is a comment
  # Indented comment

Host visible
    HostName visible.example.com
    # Inline comment after setting
    User visible_user
"#,
        );
        let parser = SshConfigParser::from_path(file.path()).unwrap().unwrap();
        let hosts = parser.get_hosts();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].user.as_deref(), Some("visible_user"));
    }

    // ── Include directive tests ─────────────────────────────────

    #[test]
    fn test_include_single_file() {
        let dir = tempfile::tempdir().unwrap();
        let main_file = dir.path().join("config");
        let include_file = dir.path().join("extra");

        write!(File::create(&include_file).unwrap(), "Host included-host\n    HostName 10.0.0.99\n    User inc\n").unwrap();
        write!(File::create(&main_file).unwrap(), "Host main-host\n    HostName 10.0.0.1\n\nInclude {}\n", include_file.display()).unwrap();

        let parser = SshConfigParser::from_path(&main_file).unwrap().unwrap();
        let hosts = parser.get_hosts();
        assert_eq!(hosts.len(), 2);

        let main = hosts.iter().find(|h| h.alias == "main-host").unwrap();
        assert_eq!(main.host_name.as_deref(), Some("10.0.0.1"));

        let inc = hosts.iter().find(|h| h.alias == "included-host").unwrap();
        assert_eq!(inc.host_name.as_deref(), Some("10.0.0.99"));
        assert_eq!(inc.user.as_deref(), Some("inc"));
    }

    #[test]
    fn test_include_glob_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let main_file = dir.path().join("config");
        let conf_dir = dir.path().join("config.d");
        std::fs::create_dir(&conf_dir).unwrap();

        write!(File::create(conf_dir.join("work.conf")).unwrap(), "Host work-server\n    HostName work.example.com\n").unwrap();
        write!(File::create(conf_dir.join("personal.conf")).unwrap(), "Host personal-server\n    HostName home.example.com\n").unwrap();
        // This file should NOT match the glob
        write!(File::create(conf_dir.join("readme.txt")).unwrap(), "not a config file\n").unwrap();

        write!(File::create(&main_file).unwrap(), "Host main\n    HostName main.example.com\n\nInclude {}/*.conf\n", conf_dir.display()).unwrap();

        let parser = SshConfigParser::from_path(&main_file).unwrap().unwrap();
        let hosts = parser.get_hosts();
        assert_eq!(hosts.len(), 3);

        let aliases: Vec<&str> = hosts.iter().map(|h| h.alias.as_str()).collect();
        assert!(aliases.contains(&"main"));
        assert!(aliases.contains(&"work-server"));
        assert!(aliases.contains(&"personal-server"));
        assert!(!aliases.contains(&"readme"));
    }

    #[test]
    fn test_include_missing_file_silent_skip() {
        let dir = tempfile::tempdir().unwrap();
        let main_file = dir.path().join("config");

        write!(File::create(&main_file).unwrap(), "Host main\n    HostName 10.0.0.1\n\nInclude /nonexistent/path/config\n").unwrap();

        let parser = SshConfigParser::from_path(&main_file).unwrap().unwrap();
        let hosts = parser.get_hosts();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "main");
    }

    #[test]
    fn test_include_max_depth() {
        // At max depth 0, Include directives should be kept as-is (no expansion)
        let content = "Host main\n    HostName 10.0.0.1\n\nInclude /nonexistent";
        let result = expand_includes(content, 0);
        assert!(result.contains("Include"));
        assert!(result.contains("Host main"));
    }

    #[test]
    fn test_glob_matches() {
        assert!(glob_matches("*.conf", "work.conf"));
        assert!(glob_matches("*.conf", "personal.conf"));
        assert!(!glob_matches("*.conf", "readme.txt"));
        assert!(glob_matches("test_?.conf", "test_a.conf"));
        assert!(!glob_matches("test_?.conf", "test_ab.conf"));
        assert!(glob_matches("*", "anything"));
        assert!(glob_matches("exact", "exact"));
        assert!(!glob_matches("exact", "different"));
    }

    // ── from_str (no include expansion) ────────────────────────

    #[test]
    fn test_from_str_no_include_expansion() {
        // from_str() parses raw content without file-system include expansion
        let config_text = "Host main\n    HostName 10.0.0.1\n\nInclude /some/path\nHost included\n    HostName 10.0.0.2\n";
        let parser = SshConfigParser::from_str(config_text).unwrap();
        let hosts = parser.get_hosts();
        // "Include /some/path" line is not a Host directive, so it's ignored by extract_host_aliases
        assert_eq!(hosts.len(), 2);
    }

    #[test]
    fn test_include_symlink_loop() {
        let dir = tempfile::tempdir().unwrap();
        let main_file = dir.path().join("config");
        let include_file = dir.path().join("extra");

        write!(File::create(&include_file).unwrap(), "Host extra-host\n    HostName 10.0.0.99\n").unwrap();

        // Create a symlink from loop.conf -> include_file (not a cycle by itself)
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&include_file, dir.path().join("loop.conf")).unwrap();
        }

        write!(
            File::create(&main_file).unwrap(),
            "Host main\n    HostName 10.0.0.1\n\nInclude {}\nInclude {}\n",
            include_file.display(),
            dir.path().join("loop.conf").display()
        ).unwrap();

        let parser = SshConfigParser::from_path(&main_file).unwrap().unwrap();
        let hosts = parser.get_hosts();
        // extra-host should appear only once despite being included twice (via direct + symlink)
        let extra_count = hosts.iter().filter(|h| h.alias == "extra-host").count();
        assert_eq!(extra_count, 1, "extra-host should appear exactly once (deduplicated via symlink cycle detection)");
    }
}
