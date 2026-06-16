//! SSH authentication module — password, private key, agent, and auto fallback.
//!
//! Provides `authenticate()` which dispatches to the correct auth method based on
//! `SshAuthMethod`. All auth operations are wrapped in a 10-second timeout.
//!
//! Electron reference: `SshConnectionManager.ts` lines 147-303
//! (`buildConnectConfig` auth section + `resolveAutoAuth`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use russh::client;
use russh::keys::load_secret_key;
use russh_keys::agent::client::AgentClient;

use crate::infrastructure::ssh_connection::agent_discovery::{mask_home_path, AgentCandidate};
use crate::types::ssh::SshAuthMethod;

/// Default authentication timeout (covers all candidates combined).
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);

/// Per-candidate timeout — prevents dead socket from blocking entire chain.
const AGENT_CANDIDATE_TIMEOUT: Duration = Duration::from_secs(3);

/// SFTP open timeout — aligns with Electron SFTP_OPEN_TIMEOUT_MS=8000.
/// Phase 3 will make this configurable via SshConfig.sftp_open_timeout_secs.
///
/// `pub(crate)` so connect_flow can access via `ssh_auth::SFTP_OPEN_TIMEOUT`.
/// (connect_flow.rs:14 has `use crate::infrastructure::ssh_auth;` module-level
/// import — constants are not auto-imported, must be qualified.)
pub(crate) const SFTP_OPEN_TIMEOUT: Duration = Duration::from_secs(8);

/// Default SSH private key paths tried during auto auth.
const DEFAULT_KEY_NAMES: &[&str] = &["id_ed25519", "id_rsa", "id_ecdsa"];

/// Error type for SSH authentication failures.
#[derive(Debug)]
pub struct AuthError {
    pub message: String,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SSH auth error: {}", self.message)
    }
}

impl std::error::Error for AuthError {}

impl AuthError {
    fn new(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

/// Expand a leading `~` in a path to the user's home directory.
///
/// If the path does not start with `~`, returns the path unchanged.
pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        match dirs::home_dir() {
            Some(home) => home.join(rest),
            None => PathBuf::from(path),
        }
    } else if path == "~" {
        match dirs::home_dir() {
            Some(home) => home,
            None => PathBuf::from(path),
        }
    } else {
        PathBuf::from(path)
    }
}

/// Get the default private key path (`~/.ssh/id_rsa`).
fn default_key_path() -> PathBuf {
    dirs::home_dir()
        .expect("HOME directory not found")
        .join(".ssh")
        .join("id_rsa")
}

// ---------------------------------------------------------------------------
// Individual auth methods
// ---------------------------------------------------------------------------

/// Authenticate with a password.
///
/// Wraps `session.authenticate_password` in a 10-second timeout.
/// In russh 0.46, `authenticate_password` returns `Result<bool, Error>`.
pub async fn auth_password<H: client::Handler>(
    session: &mut client::Handle<H>,
    username: &str,
    password: &str,
) -> Result<(), AuthError> {
    let success = tokio::time::timeout(AUTH_TIMEOUT, async {
        session
            .authenticate_password(username, password)
            .await
            .map_err(|e| AuthError::new(format!("Password auth failed: {}", e)))
    })
    .await
    .map_err(|_| AuthError::new("Password authentication timed out"))??;

    if success {
        Ok(())
    } else {
        Err(AuthError::new("Password authentication rejected"))
    }
}

/// Authenticate with a private key file.
///
/// Loads the key from `key_path` (defaults to `~/.ssh/id_rsa` if `None`),
/// then calls `session.authenticate_publickey`. Wrapped in a 10-second timeout.
/// In russh 0.46, `authenticate_publickey` takes `Arc<key::KeyPair>`.
pub async fn auth_private_key<H: client::Handler>(
    session: &mut client::Handle<H>,
    username: &str,
    key_path: Option<&str>,
) -> Result<(), AuthError> {
    let resolved_path = match key_path {
        Some(p) => expand_tilde(p),
        None => default_key_path(),
    };

    let key_path_str = resolved_path
        .to_str()
        .ok_or_else(|| AuthError::new("Invalid key path (non-UTF-8)"))?
        .to_string();

    // Load the secret key (returns key::KeyPair)
    let secret_key = load_secret_key(&key_path_str, None)
        .map_err(|e| AuthError::new(format!("Cannot read private key at {}: {}", key_path_str, e)))?;

    let success = tokio::time::timeout(AUTH_TIMEOUT, async {
        session
            .authenticate_publickey(username, Arc::new(secret_key))
            .await
            .map_err(|e| AuthError::new(format!("Public key auth failed: {}", e)))
    })
    .await
    .map_err(|_| AuthError::new("Private key authentication timed out"))??;

    if success {
        Ok(())
    } else {
        Err(AuthError::new(
            "Private key authentication rejected by server",
        ))
    }
}

/// Authenticate using the SSH agent (multi-candidate).
pub async fn auth_agent<H: client::Handler>(
    session: &mut client::Handle<H>,
    username: &str,
    agent_sockets: &[AgentCandidate],
) -> Result<(), AuthError> {
    // Outer 10s timeout — rarely triggers since per-candidate 3s limits
    // each attempt. If triggered, include partial errors from inner call.
    match tokio::time::timeout(AUTH_TIMEOUT, do_auth_agent_multi(session, username, agent_sockets)).await {
        Ok(inner) => inner,
        Err(_) => Err(AuthError::new(format!(
            "SSH agent authentication timed out after {}s (tried {} candidates)",
            AUTH_TIMEOUT.as_secs(),
            agent_sockets.len()
        ))),
    }
}

/// Iterate over multiple agent candidates. Per-candidate 3s timeout prevents
/// dead socket from blocking chain. errors: Vec<String> accumulates all failures.
async fn do_auth_agent_multi<H: client::Handler>(
    session: &mut client::Handle<H>,
    username: &str,
    agent_sockets: &[AgentCandidate],
) -> Result<(), AuthError> {
    if agent_sockets.is_empty() {
        return Err(AuthError::new("No SSH agent sockets available"));
    }

    let mut errors: Vec<String> = Vec::new();

    for candidate in agent_sockets {
        let masked_path = mask_home_path(&candidate.path);

        let attempt = tokio::time::timeout(
            AGENT_CANDIDATE_TIMEOUT,
            try_agent_candidate(session, username, candidate),
        )
        .await;

        match attempt {
            Ok(Ok(())) => {
                log::info!("Agent auth succeeded via [{}] {}", candidate.source, masked_path);
                return Ok(());
            }
            Ok(Err(e)) => {
                log::debug!("[{}] {} failed: {}", candidate.source, masked_path, e);
                errors.push(format!("[{}] {}: {}", candidate.source, masked_path, e));
            }
            Err(_) => {
                log::debug!(
                    "[{}] {} timed out after {}s",
                    candidate.source, masked_path, AGENT_CANDIDATE_TIMEOUT.as_secs()
                );
                errors.push(format!(
                    "[{}] {}: timed out after {}s",
                    candidate.source, masked_path, AGENT_CANDIDATE_TIMEOUT.as_secs()
                ));
            }
        }
    }

    Err(AuthError::new(format!(
        "All {} agent candidates failed: {}",
        agent_sockets.len(),
        errors.join("; ")
    )))
}

/// Try one agent candidate: connect → request identities → try each.
/// Symmetric naming with phase 1 try_key_auth.
async fn try_agent_candidate<H: client::Handler>(
    session: &mut client::Handle<H>,
    username: &str,
    candidate: &AgentCandidate,
) -> Result<(), AuthError> {
    let masked = mask_home_path(&candidate.path);

    let mut agent = AgentClient::connect_uds(&candidate.path)
        .await
        .map_err(|e| AuthError::new(format!("connect {} failed: {}", masked, e)))?;

    let identities = agent
        .request_identities()
        .await
        .map_err(|e| AuthError::new(format!("identities {} failed: {}", masked, e)))?;

    if identities.is_empty() {
        return Err(AuthError::new(format!("{}: no identities", masked)));
    }

    let mut last_err = AuthError::new("No identities tried");
    for identity in identities {
        let fp = identity.fingerprint();
        let agent_inner = match AgentClient::connect_uds(&candidate.path).await {
            Ok(a) => a,
            Err(e) => {
                last_err = AuthError::new(format!("reconnect {} failed: {}", masked, e));
                break;
            }
        };

        let (_returned_agent, auth_result) = session
            .authenticate_future(username, identity, agent_inner)
            .await;

        match auth_result {
            Ok(true) => return Ok(()),
            Ok(false) => last_err = AuthError::new(format!("identity {} rejected", fp)),
            Err(e) => {
                log::debug!("Auth error for identity {}: {}", fp, e);
                last_err = AuthError::new(format!("identity {} error", fp));
            }
        }
    }

    Err(last_err)
}

/// Try authenticating with a single key file.
///
/// Returns `Ok(())` on success, `Err` with a description on failure.
/// Does not wrap in a timeout (caller should handle that).
pub async fn try_key_auth<H: client::Handler>(
    session: &mut client::Handle<H>,
    username: &str,
    key_path: &Path,
) -> Result<(), AuthError> {
    let key_path_str = key_path
        .to_str()
        .ok_or_else(|| AuthError::new("Invalid key path (non-UTF-8)"))?;

    if !key_path.exists() {
        return Err(AuthError::new(format!(
            "Key file not found: {}",
            key_path_str
        )));
    }

    let secret_key = load_secret_key(key_path_str, None).map_err(|e| {
        AuthError::new(format!(
            "Cannot read private key at {}: {}",
            key_path_str, e
        ))
    })?;

    let success = session
        .authenticate_publickey(username, Arc::new(secret_key))
        .await
        .map_err(|e| {
            AuthError::new(format!(
                "Public key auth failed for {}: {}",
                key_path_str, e
            ))
        })?;

    if success {
        Ok(())
    } else {
        Err(AuthError::new(format!(
            "Key auth rejected for {}",
            key_path_str
        )))
    }
}

/// Authenticate with auto fallback (mirrors Electron `resolveAutoAuth`).
///
/// Phase 2 signature: identity_files from ssh -G + multi-candidate agent_sockets.
/// HashSet<PathBuf> tracks tried paths — avoids re-trying default keys
/// (ssh -G always returns id_ed25519/id_rsa/id_ecdsa by default).
pub async fn auth_auto<H: client::Handler>(
    session: &mut client::Handle<H>,
    username: &str,
    identity_files: &[PathBuf],
    agent_sockets: &[AgentCandidate],
) -> Result<(), AuthError> {
    let ssh_dir = match dirs::home_dir() {
        Some(home) => home.join(".ssh"),
        None => {
            return Err(AuthError::new(
                "Cannot determine home directory for auto auth (set $HOME or use explicit PrivateKey path)",
            ))
        }
    };

    let mut tried_paths: HashSet<PathBuf> = HashSet::new();
    let mut tried_steps: Vec<&str> = Vec::new();

    // Step 1: identity files
    for key_path in identity_files {
        if !tried_paths.insert(key_path.clone()) {
            log::debug!("Skipping duplicate identity file: {:?}", key_path);
            continue;
        }
        match try_key_auth_with_timeout(session, username, key_path).await {
            Ok(()) => {
                log::info!("Auto auth succeeded with identity file: {:?}", key_path);
                return Ok(());
            }
            Err(e) => log::debug!("Identity file {:?} failed: {}", key_path, e),
        }
    }
    if !identity_files.is_empty() {
        tried_steps.push("identity files");
    }

    // Step 2: agent sockets
    if !agent_sockets.is_empty() {
        match auth_agent(session, username, agent_sockets).await {
            Ok(()) => {
                log::info!("Auto auth succeeded with SSH agent");
                return Ok(());
            }
            Err(e) => log::debug!("All agent sockets failed: {}", e),
        }
        tried_steps.push("agent sockets");
    }

    // Step 3: default keys (skip already-tried)
    let mut default_tried = 0;
    for key_name in DEFAULT_KEY_NAMES {
        let key_path = ssh_dir.join(key_name);
        if !tried_paths.insert(key_path.clone()) {
            continue;
        }
        if try_key_auth_with_timeout(session, username, &key_path).await.is_ok() {
            log::info!("Auto auth succeeded with default key: {}", key_name);
            return Ok(());
        }
        default_tried += 1;
    }
    if default_tried > 0 {
        tried_steps.push("default keys");
    }

    Err(AuthError::new(format!(
        "Auto authentication failed (tried: {})",
        if tried_steps.is_empty() { "no candidates available".to_string() } else { tried_steps.join(", ") }
    )))
}

/// Wrap `try_key_auth` with the standard 10-second timeout.
async fn try_key_auth_with_timeout<H: client::Handler>(
    session: &mut client::Handle<H>,
    username: &str,
    key_path: &Path,
) -> Result<(), AuthError> {
    tokio::time::timeout(AUTH_TIMEOUT, try_key_auth(session, username, key_path))
        .await
        .map_err(|_| AuthError::new(format!("Key auth timed out for {:?}", key_path)))?
}

// ---------------------------------------------------------------------------
// Dispatcher
// ---------------------------------------------------------------------------

/// Build SFTP-open timeout diagnostic. IPv6 hosts get bracketed to avoid
/// ssh parsing ambiguity (sftp user@2001:db8::1 would parse port as db8::1).
///
/// **No backticks** in message — `whitespace-pre-line` renders them as literal
/// characters, not <code>. Phase 2 uses plain text.
pub fn build_sftp_timeout_msg(user: &str, host: &str, secs: u64) -> String {
    let host_part = if host.contains(':') {
        format!("[{}]", host)
    } else {
        host.to_string()
    };
    format!(
        "SFTP subsystem unavailable (timed out after {}s).\n\
         Likely causes:\n\
         \x20\x20• Server sshd_config missing Subsystem sftp directive\n\
         \x20\x20• Account in restricted shell (rbash) or ChrootDirectory blocking SFTP\n\
         Reproduce with: sftp {}@{}",
        secs, user, host_part
    )
}

/// Dispatch to the appropriate authentication method.
///
/// # Arguments (changed in phase 2)
/// * `session` - Active russh `Handle` (post-connect)
/// * `username` - SSH username
/// * `method` - Auth method from `SshConnectionConfig`
/// * `password` - Password (used only when `method == Password`)
/// * `private_key_path` - Path to private key (used when `method == PrivateKey`)
/// * `identity_files` - Identity files from ssh -G (used when `method == Auto`)
/// * `agent_sockets` - Multi-candidate agent sockets (used when `method == Agent` or `Auto`)
///
/// # Removed in phase 2
/// * `config_parser: Option<&SshConfigParser>` — identity_files now passed directly
/// * `resolved_alias: Option<&str>` — host_resolver resolves alias internally
pub async fn authenticate<H: client::Handler>(
    session: &mut client::Handle<H>,
    username: &str,
    method: &SshAuthMethod,
    password: Option<&str>,
    private_key_path: Option<&str>,
    identity_files: &[PathBuf],
    agent_sockets: &[AgentCandidate],
) -> Result<(), AuthError> {
    match method {
        SshAuthMethod::Password => {
            let pwd = password.ok_or_else(|| {
                AuthError::new("Password auth method selected but no password provided")
            })?;
            auth_password(session, username, pwd).await
        }
        SshAuthMethod::PrivateKey => auth_private_key(session, username, private_key_path).await,
        SshAuthMethod::Agent => auth_agent(session, username, agent_sockets).await,
        SshAuthMethod::Auto => {
            auth_auto(session, username, identity_files, agent_sockets).await
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_tilde_with_home() {
        let result = expand_tilde("~/Documents/file.txt");
        let home = dirs::home_dir().unwrap();
        assert_eq!(result, home.join("Documents/file.txt"));
    }

    #[test]
    fn test_expand_tilde_bare() {
        let result = expand_tilde("~");
        let home = dirs::home_dir().unwrap();
        assert_eq!(result, home);
    }

    #[test]
    fn test_expand_tilde_no_tilde() {
        let result = expand_tilde("/absolute/path/file.txt");
        assert_eq!(result, PathBuf::from("/absolute/path/file.txt"));
    }

    #[test]
    fn test_expand_tilde_relative_path() {
        let result = expand_tilde("relative/path");
        assert_eq!(result, PathBuf::from("relative/path"));
    }

    #[test]
    fn test_expand_tilde_tilde_in_middle() {
        // Tilde in the middle of a path should NOT be expanded
        let result = expand_tilde("/path/to/~user/file");
        assert_eq!(result, PathBuf::from("/path/to/~user/file"));
    }

    #[test]
    fn test_default_key_path() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(default_key_path(), home.join(".ssh").join("id_rsa"));
    }

    #[test]
    fn test_build_sftp_timeout_msg_basic() {
        let msg = build_sftp_timeout_msg("alice", "example.com", 8);
        assert!(msg.contains("timed out after 8s"));
        assert!(msg.contains("Subsystem sftp"));
        assert!(msg.contains("restricted shell"));
        assert!(msg.contains("sftp alice@example.com"));
    }

    #[test]
    fn test_build_sftp_timeout_msg_ipv6_gets_brackets() {
        let msg = build_sftp_timeout_msg("alice", "2001:db8::1", 8);
        assert!(msg.contains("sftp alice@[2001:db8::1]"));
        assert!(!msg.contains("alice@2001:db8::1]")); // no malformed variant
    }
}
