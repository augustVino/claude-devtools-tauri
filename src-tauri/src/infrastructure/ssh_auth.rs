//! SSH authentication module — password, private key, agent, and auto fallback.
//!
//! Provides `authenticate()` which dispatches to the correct auth method based on
//! `SshAuthMethod`. All auth operations are wrapped in a 10-second timeout.
//!
//! Electron reference: `SshConnectionManager.ts` lines 147-303
//! (`buildConnectConfig` auth section + `resolveAutoAuth`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use russh::client;
use russh::keys::load_secret_key;
use russh_keys::agent::client::AgentClient;

use crate::infrastructure::ssh_config_parser::SshConfigParser;
use crate::types::ssh::SshAuthMethod;

/// Default authentication timeout (10 seconds).
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);

/// Default SSH private key paths tried during auto auth.
const DEFAULT_KEY_NAMES: &[&str] = &["id_ed25519", "id_rsa", "id_ecdsa"];

/// SSH config identity file key paths tried first in auto auth.
const CONFIG_KEY_NAMES: &[&str] = &["id_ed25519", "id_rsa"];

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

/// Authenticate using the SSH agent.
///
/// Connects to the local SSH agent via `SSH_AUTH_SOCK`, lists identities,
/// and tries each one using `authenticate_future` (the Signer-based API).
/// Wrapped in a 10-second timeout.
pub async fn auth_agent<H: client::Handler>(
    session: &mut client::Handle<H>,
    username: &str,
) -> Result<(), AuthError> {
    let result = tokio::time::timeout(AUTH_TIMEOUT, do_auth_agent(session, username)).await;

    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(AuthError::new("SSH agent authentication timed out")),
    }
}

/// Internal implementation for agent auth.
///
/// Uses `AgentClient::connect_env()` to connect to the SSH agent,
/// `request_identities()` to list keys, and `session.authenticate_future()`
/// to perform agent-based authentication (the agent handles signing).
async fn do_auth_agent<H: client::Handler>(
    session: &mut client::Handle<H>,
    username: &str,
) -> Result<(), AuthError> {
    let mut agent = AgentClient::connect_env()
        .await
        .map_err(|e| AuthError::new(format!("Cannot connect to SSH agent: {}", e)))?;

    let identities = agent
        .request_identities()
        .await
        .map_err(|e| AuthError::new(format!("Failed to request agent identities: {}", e)))?;

    if identities.is_empty() {
        return Err(AuthError::new("SSH agent has no identities loaded"));
    }

    let mut last_error = String::from("No identities to try");

    for identity in identities {
        // Compute fingerprint before moving identity into authenticate_future
        let fp = identity.fingerprint();

        // authenticate_future takes the AgentClient (Signer impl) and a public key.
        // The agent handles the actual signing internally.
        let agent_inner = AgentClient::connect_env()
            .await
            .map_err(|e| AuthError::new(format!("Cannot reconnect to SSH agent: {}", e)))?;

        let (_returned_agent, auth_result) = session
            .authenticate_future(username, identity, agent_inner)
            .await;

        match auth_result {
            Ok(true) => return Ok(()),
            Ok(false) => {
                last_error = format!("Identity {} rejected by server", fp);
            }
            Err(e) => {
                last_error = format!("Error authenticating with identity: {}", fp);
                log::debug!("Agent auth error for identity: {}", e);
            }
        }
    }

    Err(AuthError::new(format!(
        "All agent identities failed: {}",
        last_error
    )))
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
/// Tries the following in order:
/// 1. If SSH config has `IdentityFile` -> try `id_ed25519`, `id_rsa`
/// 2. SSH agent
/// 3. Default keys: `id_ed25519`, `id_rsa`, `id_ecdsa`
/// 4. All failed -> Err
pub async fn auth_auto<H: client::Handler>(
    session: &mut client::Handle<H>,
    username: &str,
    config_parser: Option<&SshConfigParser>,
    resolved_alias: Option<&str>,
) -> Result<(), AuthError> {
    let ssh_dir = match dirs::home_dir() {
        Some(home) => home.join(".ssh"),
        None => {
            return Err(AuthError::new(
                "Cannot determine home directory for auto auth",
            ))
        }
    };

    // Step 1: If SSH config has IdentityFile, try config identity keys first
    if let (Some(parser), Some(alias)) = (config_parser, resolved_alias) {
        if let Some(entry) = parser.resolve_host(alias) {
            if entry.has_identity_file {
                for key_name in CONFIG_KEY_NAMES {
                    let key_path = ssh_dir.join(key_name);
                    if try_key_auth_with_timeout(session, username, &key_path).await.is_ok() {
                        log::info!(
                            "Auto auth succeeded with config identity key: {}",
                            key_name
                        );
                        return Ok(());
                    }
                }
            }
        }
    }

    // Step 2: Try SSH agent
    if auth_agent(session, username).await.is_ok() {
        log::info!("Auto auth succeeded with SSH agent");
        return Ok(());
    }

    // Step 3: Try default keys
    for key_name in DEFAULT_KEY_NAMES {
        let key_path = ssh_dir.join(key_name);
        if try_key_auth_with_timeout(session, username, &key_path).await.is_ok() {
            log::info!("Auto auth succeeded with default key: {}", key_name);
            return Ok(());
        }
    }

    Err(AuthError::new(
        "Auto authentication failed: tried SSH config keys, agent, and default keys",
    ))
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

/// Dispatch to the appropriate authentication method.
///
/// This is the main entry point called by `SshConnectionManager` after
/// establishing a TCP+SSH connection.
///
/// # Arguments
/// * `session` - Active russh `Handle` (post-connect)
/// * `username` - SSH username
/// * `method` - Auth method from `SshConnectionConfig`
/// * `password` - Password (used only when `method == Password`)
/// * `private_key_path` - Path to private key (used when `method == PrivateKey`)
/// * `config_parser` - SSH config parser (used when `method == Auto`)
/// * `resolved_alias` - Original host alias (used when `method == Auto`)
pub async fn authenticate<H: client::Handler>(
    session: &mut client::Handle<H>,
    username: &str,
    method: &SshAuthMethod,
    password: Option<&str>,
    private_key_path: Option<&str>,
    config_parser: Option<&SshConfigParser>,
    resolved_alias: Option<&str>,
) -> Result<(), AuthError> {
    match method {
        SshAuthMethod::Password => {
            let pwd = password.ok_or_else(|| {
                AuthError::new("Password auth method selected but no password provided")
            })?;
            auth_password(session, username, pwd).await
        }
        SshAuthMethod::PrivateKey => {
            auth_private_key(session, username, private_key_path).await
        }
        SshAuthMethod::Agent => {
            auth_agent(session, username).await
        }
        SshAuthMethod::Auto => {
            auth_auto(session, username, config_parser, resolved_alias).await
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
}
