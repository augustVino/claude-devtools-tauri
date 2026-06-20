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
use crate::infrastructure::ssh_connection::auth_trace::{AttemptOutcome, AuthTrace};
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
    /// Phase 3a: structured trace for error enrichment.
    /// Empty trace (Default) means "no context collected" — caller used
    /// `AuthError::new()` convenience constructor.
    pub trace: AuthTrace,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // If trace is empty, display just message (backward compat with phase 2).
        // Otherwise render enriched multi-section message.
        if self.trace.attempts.is_empty()
            && self.trace.timings.resolve_ms == 0
            && self.trace.timings.tcp_probe_ms == 0
            && self.trace.timings.tcp_handshake_ms.is_none()
            && self.trace.timings.auth_attempts_ms.is_empty()
        {
            write!(f, "SSH auth error: {}", self.message)
        } else {
            use crate::infrastructure::ssh_connection::auth_trace::enrich_auth_error;
            write!(f, "{}", enrich_auth_error(&self.message, &self.trace))
        }
    }
}

impl std::error::Error for AuthError {}

impl AuthError {
    /// Construct without trace (phase 2 backward compat).
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            trace: AuthTrace::new(),
        }
    }

    /// Construct with trace (phase 3a).
    pub fn with_trace(msg: impl Into<String>, trace: AuthTrace) -> Self {
        Self {
            message: msg.into(),
            trace,
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

/// Resolve default private key path from a given home directory.
///
/// Extracted as a pure function for testability (no env mutation).
/// Returns `AuthError` if `home` is `None` instead of panicking.
fn build_default_key_path(home: Option<&std::path::Path>) -> Result<PathBuf, AuthError> {
    let home = home.ok_or_else(|| {
        AuthError::new(
            "Cannot resolve default SSH key path: $HOME not set. \
             Set $HOME environment variable, or specify config.private_key_path explicitly.",
        )
    })?;
    Ok(home.join(".ssh").join("id_rsa"))
}

/// Get the default private key path (`~/.ssh/id_rsa`).
///
/// Wrapper around `build_default_key_path` reading $HOME via `dirs`.
/// Returns `AuthError` in containerized/sandboxed environments without $HOME.
fn default_key_path() -> Result<PathBuf, AuthError> {
    build_default_key_path(dirs::home_dir().as_deref())
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
        None => default_key_path()?,
    };

    let key_path_str = resolved_path
        .to_str()
        .ok_or_else(|| AuthError::new("Invalid key path (non-UTF-8)"))?
        .to_string();

    // Load the secret key (returns key::KeyPair)
    let secret_key = load_secret_key(&key_path_str, None).map_err(|e| {
        AuthError::new(format!(
            "Cannot read private key at {}: {}",
            key_path_str, e
        ))
    })?;

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
    trace: &mut AuthTrace,
) -> Result<(), AuthError> {
    // Outer 10s timeout — rarely triggers since per-candidate 3s limits
    // each attempt. If triggered, include partial errors from inner call.
    match tokio::time::timeout(
        AUTH_TIMEOUT,
        do_auth_agent_multi(session, username, agent_sockets, trace),
    )
    .await
    {
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
/// Phase 3a: records each attempt (source + outcome + ms) into `trace`.
async fn do_auth_agent_multi<H: client::Handler>(
    session: &mut client::Handle<H>,
    username: &str,
    agent_sockets: &[AgentCandidate],
    trace: &mut AuthTrace,
) -> Result<(), AuthError> {
    if agent_sockets.is_empty() {
        return Err(AuthError::new("No SSH agent sockets available"));
    }

    let mut errors: Vec<String> = Vec::new();

    for candidate in agent_sockets {
        let masked_path = mask_home_path(&candidate.path);
        let source_label = format!("{} {}", candidate.source, masked_path);
        let attempt_start = std::time::Instant::now();

        let attempt = tokio::time::timeout(
            AGENT_CANDIDATE_TIMEOUT,
            try_agent_candidate(session, username, candidate),
        )
        .await;

        let attempt_ms = attempt_start.elapsed().as_millis() as u64;

        match attempt {
            Ok(Ok(())) => {
                log::info!(
                    "Agent auth succeeded via [{}] {}",
                    candidate.source,
                    masked_path
                );
                trace.record_attempt(source_label, AttemptOutcome::Used, attempt_ms);
                return Ok(());
            }
            Ok(Err(e)) => {
                let reason = e.to_string();
                log::debug!("[{}] {} failed: {}", candidate.source, masked_path, reason);
                trace.record_attempt(
                    source_label,
                    AttemptOutcome::Failed {
                        reason: reason.clone(),
                    },
                    attempt_ms,
                );
                errors.push(format!(
                    "[{}] {}: {}",
                    candidate.source, masked_path, reason
                ));
            }
            Err(_) => {
                let reason = format!("timed out after {}s", AGENT_CANDIDATE_TIMEOUT.as_secs());
                log::debug!("[{}] {} {}", candidate.source, masked_path, reason);
                trace.record_attempt(
                    source_label,
                    AttemptOutcome::Failed {
                        reason: reason.clone(),
                    },
                    attempt_ms,
                );
                errors.push(format!(
                    "[{}] {}: {}",
                    candidate.source, masked_path, reason
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

    // Mask $HOME prefix for privacy-safe error messages forwarded to frontend.
    // NOTE: fs operations (key_path.exists(), load_secret_key) still use the
    // unmasked `key_path` / `key_path_str` — only log/error strings use masked.
    let masked_path = mask_home_path(key_path);

    if !key_path.exists() {
        return Err(AuthError::new(format!(
            "Key file not found: {}",
            masked_path
        )));
    }

    let secret_key = load_secret_key(key_path_str, None).map_err(|e| {
        AuthError::new(format!("Cannot read private key at {}: {}", masked_path, e))
    })?;

    let success = session
        .authenticate_publickey(username, Arc::new(secret_key))
        .await
        .map_err(|e| {
            AuthError::new(format!("Public key auth failed for {}: {}", masked_path, e))
        })?;

    if success {
        Ok(())
    } else {
        Err(AuthError::new(format!(
            "Key auth rejected for {}",
            masked_path
        )))
    }
}

/// 认证候选步骤的纯数据表示。生产代码 auth_auto 通过此枚举迭代执行，
/// 顺序由 plan_auth_candidate_steps 决定（single source of truth）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AuthStep {
    Agent,
    IdentityFile(PathBuf),
    DefaultKey(&'static str),
}

/// 规划 auth_auto 的候选步骤顺序（纯函数，不执行真认证）。
///
/// 对齐 Electron `buildAuthCandidates` 顺序：
/// **agent sockets → identity files → default keys**.
///
/// `has_agent` 而非 `&[AgentCandidate]` 的原因：纯函数只关心顺序，
/// 不关心 agent 候选的内部结构（AgentSource + PathBuf）。调用方
/// `auth_auto` 传入 `!agent_sockets.is_empty()` 即可。
///
/// 此函数是 auth_auto 顺序的唯一来源 —— auth_auto 迭代此函数返回的 Vec<AuthStep>
/// 执行实际认证，测试通过断言返回值守护顺序不变。
pub(crate) fn plan_auth_candidate_steps(
    has_agent: bool,
    identity_files: &[PathBuf],
) -> Vec<AuthStep> {
    let mut steps = Vec::new();
    if has_agent {
        steps.push(AuthStep::Agent);
    }
    for f in identity_files {
        steps.push(AuthStep::IdentityFile(f.clone()));
    }
    for name in DEFAULT_KEY_NAMES {
        steps.push(AuthStep::DefaultKey(name));
    }
    steps
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
    trace: &mut AuthTrace,
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
    // tried_steps 用 Vec + contains 检查去重（修正 codex 第四轮 C4）：
    // 保留实际尝试顺序，避免 HashSet sort 破坏 "agent sockets < identity files < default keys" 语义。
    // 协议规则七："测试验证意图而非仅行为"——错误消息的意图是反映尝试顺序。
    let mut tried_steps: Vec<&'static str> = Vec::new();
    /// 内联辅助：去重 push
    fn record_step(tried_steps: &mut Vec<&'static str>, step: &'static str) {
        if !tried_steps.contains(&step) {
            tried_steps.push(step);
        }
    }

    // 顺序由 plan_auth_candidate_steps 决定（生产模块纯函数，测试守护）：
    // agent sockets → identity files → default keys
    //
    // 注意：ssh_dir 来自函数顶部（417-424 行），保留不变。
    // plan_auth_candidate_steps 第一个参数是 has_agent: bool（避免 AgentCandidate 类型耦合）。
    let plan = plan_auth_candidate_steps(!agent_sockets.is_empty(), identity_files);

    for step in plan {
        match step {
            AuthStep::Agent => {
                match auth_agent(session, username, agent_sockets, trace).await {
                    Ok(()) => {
                        log::info!("Auto auth succeeded with SSH agent");
                        return Ok(());
                    }
                    Err(e) => log::debug!("All agent sockets failed: {}", e),
                }
                record_step(&mut tried_steps, "agent sockets");
            }
            AuthStep::IdentityFile(key_path) => {
                let masked = mask_home_path(&key_path);
                if !tried_paths.insert(key_path.clone()) {
                    log::debug!("Skipping duplicate identity file: {}", masked);
                    trace.record_attempt(
                        masked.clone(),
                        AttemptOutcome::Skipped {
                            reason: "duplicate path".to_string(),
                        },
                        0,
                    );
                    continue;
                }
                let attempt_start = std::time::Instant::now();
                match try_key_auth_with_timeout(session, username, &key_path).await {
                    Ok(()) => {
                        let ms = attempt_start.elapsed().as_millis() as u64;
                        log::info!("Auto auth succeeded with identity file: {}", masked);
                        trace.record_attempt(masked, AttemptOutcome::Used, ms);
                        return Ok(());
                    }
                    Err(e) => {
                        let ms = attempt_start.elapsed().as_millis() as u64;
                        let reason = e.to_string();
                        log::debug!("Identity file {} failed: {}", masked, reason);
                        // 加密 key 用 Skipped 而非 Failed（与 Task 6 协调）
                        let outcome = if reason.contains("skipped — passphrases") {
                            record_step(&mut tried_steps, "encrypted keys (skipped)");
                            AttemptOutcome::Skipped {
                                reason: "encrypted key — use ssh-agent".to_string(),
                            }
                        } else {
                            AttemptOutcome::Failed { reason }
                        };
                        trace.record_attempt(masked, outcome, ms);
                    }
                }
                record_step(&mut tried_steps, "identity files");
            }
            // 修正 codex 第四轮 H1：DefaultKey arm 加 Skipped 分支（与 IdentityFile 对称）
            // 用户可能把加密 key 放在 ~/.ssh/id_rsa 等默认位置，需要同样友好处理
            AuthStep::DefaultKey(name) => {
                // ssh_dir 来自函数顶部（417-424 行的 dirs::home_dir().join(".ssh")）
                let key_path = ssh_dir.join(name);
                let masked = mask_home_path(&key_path);
                if !tried_paths.insert(key_path.clone()) {
                    trace.record_attempt(
                        masked,
                        AttemptOutcome::Skipped {
                            reason: "duplicate path".to_string(),
                        },
                        0,
                    );
                    continue;
                }
                let attempt_start = std::time::Instant::now();
                // 用 match 替代 is_ok()，保留错误细节（含 "Key file not found" 等）
                match try_key_auth_with_timeout(session, username, &key_path).await {
                    Ok(()) => {
                        let ms = attempt_start.elapsed().as_millis() as u64;
                        log::info!("Auto auth succeeded with default key: {}", name);
                        trace.record_attempt(masked, AttemptOutcome::Used, ms);
                        return Ok(());
                    }
                    Err(e) => {
                        let ms = attempt_start.elapsed().as_millis() as u64;
                        let reason = e.to_string();
                        log::debug!("Default key {} failed: {}", name, reason);
                        let outcome = if reason.contains("skipped — passphrases") {
                            record_step(&mut tried_steps, "encrypted keys (skipped)");
                            AttemptOutcome::Skipped {
                                reason: "encrypted key — use ssh-agent".to_string(),
                            }
                        } else {
                            AttemptOutcome::Failed { reason }
                        };
                        trace.record_attempt(masked, outcome, ms);
                    }
                }
                record_step(&mut tried_steps, "default keys");
            }
        }
    }

    Err(AuthError::new(format!(
        "Auto authentication failed (tried: {})",
        if tried_steps.is_empty() {
            "no candidates available".to_string()
        } else {
            tried_steps.join(", ")
        }
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
        .map_err(|_| {
            AuthError::new(format!(
                "Key auth timed out for {}",
                mask_home_path(key_path)
            ))
        })?
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
    trace: &mut AuthTrace,
) -> Result<(), AuthError> {
    match method {
        SshAuthMethod::Password => {
            let pwd = password.ok_or_else(|| {
                AuthError::new("Password auth method selected but no password provided")
            })?;
            auth_password(session, username, pwd).await
        }
        SshAuthMethod::PrivateKey => auth_private_key(session, username, private_key_path).await,
        SshAuthMethod::Agent => auth_agent(session, username, agent_sockets, trace).await,
        SshAuthMethod::Auto => {
            auth_auto(session, username, identity_files, agent_sockets, trace).await
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
        assert_eq!(
            default_key_path().unwrap(),
            home.join(".ssh").join("id_rsa")
        );
    }

    #[test]
    fn test_build_default_key_path_with_home() {
        let home = std::path::Path::new("/fake/home");
        let result = build_default_key_path(Some(home));
        assert_eq!(
            result.unwrap(),
            std::path::PathBuf::from("/fake/home/.ssh/id_rsa")
        );
    }

    #[test]
    fn test_build_default_key_path_without_home_returns_err() {
        let result = build_default_key_path(None);
        assert!(result.is_err(), "should return AuthError when home is None");
        let err_msg = result.unwrap_err().message;
        assert!(
            err_msg.contains("$HOME"),
            "error should mention $HOME: {}",
            err_msg
        );
        assert!(
            err_msg.contains("private_key_path"),
            "error should guide to config.private_key_path: {}",
            err_msg
        );
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

    /// Phase 3a: AuthError Display with empty trace must match phase 2 format.
    #[test]
    fn test_auth_error_display_no_trace_returns_phase2_format() {
        let err = AuthError::new("password rejected");
        assert_eq!(err.to_string(), "SSH auth error: password rejected");
    }

    /// Phase 3a: AuthError Display with non-empty trace triggers enrich_auth_error.
    #[test]
    fn test_auth_error_display_with_trace_renders_enriched() {
        let mut trace = AuthTrace::new();
        trace.record_attempt(
            "~/.ssh/id_ed25519",
            AttemptOutcome::Failed {
                reason: "rejected".to_string(),
            },
            100,
        );
        let err = AuthError::with_trace("auth failed", trace);
        let displayed = err.to_string();
        assert!(displayed.contains("auth failed"));
        assert!(displayed.contains("Auth chain:"));
        assert!(displayed.contains("~/.ssh/id_ed25519 — failed (rejected)"));
        assert!(displayed.contains("Timing:"));
    }

    /// Phase 3a: AgentSource label() must match phase 2 string literals.
    #[test]
    fn test_agent_source_label_matches_phase2_strings() {
        use crate::infrastructure::ssh_connection::agent_discovery::AgentSource;
        let cases: [(AgentSource, &str); 9] = [
            (AgentSource::IdentityAgent, "IdentityAgent"),
            (AgentSource::EnvSshAuthSock, "SSH_AUTH_SOCK"),
            (
                AgentSource::OnePasswordAppStoreTSub,
                "1Password AppStore (t/agent.sock)",
            ),
            (
                AgentSource::OnePasswordAppStore,
                "1Password AppStore (agent.sock)",
            ),
            (AgentSource::OnePasswordCli, "1Password CLI"),
            (AgentSource::Launchctl, "launchctl SSH_AUTH_SOCK"),
            (AgentSource::HomeSshAgentSock, "~/.ssh/agent.sock"),
            (AgentSource::SystemdAgent, "systemd ssh-agent.socket"),
            (AgentSource::GnomeKeyring, "gnome-keyring ssh"),
        ];
        for (variant, expected) in cases.iter() {
            assert_eq!(
                variant.label(),
                *expected,
                "label mismatch for {:?}",
                variant
            );
            assert_eq!(
                variant.to_string(),
                *expected,
                "Display mismatch for {:?}",
                variant
            );
        }
    }

    /// Phase 3a: AuthError::with_trace carries trace through Display.
    #[test]
    fn test_auth_error_with_trace_preserves_trace_in_display() {
        let mut trace = AuthTrace::new();
        trace.timings.resolve_ms = 42;
        let err = AuthError::with_trace("early failure", trace);
        assert!(err.to_string().contains("resolve: 42ms"));
    }

    /// Task 2: agent must come first when both agent and identity_files present.
    #[test]
    fn test_plan_auth_candidate_steps_agent_first_when_both_present() {
        let files = vec![std::path::PathBuf::from("/home/u/.ssh/id_ed25519")];
        let steps = super::plan_auth_candidate_steps(true, &files);
        assert!(!steps.is_empty());
        assert!(
            matches!(steps[0], super::AuthStep::Agent),
            "agent must come first when both agent and identity_files present"
        );
        let first_file_idx = steps
            .iter()
            .position(|s| matches!(s, super::AuthStep::IdentityFile(_)))
            .expect("should have identity file step");
        let default_idx = steps
            .iter()
            .position(|s| matches!(s, super::AuthStep::DefaultKey(_)))
            .expect("should have default key step");
        assert!(
            first_file_idx < default_idx,
            "identity files must come before default keys"
        );
    }

    /// Task 2: only agent → agent first, then default keys (no identity files).
    #[test]
    fn test_plan_auth_candidate_steps_only_agent() {
        let steps = super::plan_auth_candidate_steps(true, &[]);
        assert!(matches!(steps[0], super::AuthStep::Agent));
        assert!(steps.len() > 1);
        assert!(matches!(steps[1], super::AuthStep::DefaultKey(_)));
    }

    /// Task 2: no agent → identity file first; agent arm must not appear.
    #[test]
    fn test_plan_auth_candidate_steps_only_files_when_no_agent() {
        let files = vec![std::path::PathBuf::from("/home/u/.ssh/company_key")];
        let steps = super::plan_auth_candidate_steps(false, &files);
        assert!(
            matches!(steps[0], super::AuthStep::IdentityFile(_)),
            "identity file should be first when no agent"
        );
        assert!(!steps
            .iter()
            .any(|s| matches!(s, super::AuthStep::Agent)));
    }
}
