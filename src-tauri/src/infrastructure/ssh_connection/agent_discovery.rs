//! SSH Agent socket discovery.
//!
//! Phase 1: discover_agent_socket_static() — single-candidate, sync.
//! Phase 2: discover_agent_sockets() — multi-candidate, async, with dedup.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use crate::infrastructure::ssh_auth::expand_tilde;

/// A discovered agent socket candidate with provenance label.
#[derive(Debug, Clone)]
pub(super) struct AgentCandidate {
    /// Provenance: "IdentityAgent" / "SSH_AUTH_SOCK" / ...
    /// (Phase 3 will make this an enum.)
    pub source: String,
    pub path: PathBuf,
}

/// Mask $HOME prefix in a path string for log/error output (privacy).
///
/// `/Users/alice/Library/.../agent.sock` → `~/Library/.../agent.sock`
pub(super) fn mask_home_path(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(stripped) = path.strip_prefix(&home) {
            return format!("~/{}", stripped.display());
        }
    }
    path.display().to_string()
}

/// Deduplicate candidates by canonical path (symlinks resolved).
/// First by input order wins. If canonicalize fails (broken symlink),
/// falls back to raw path.
pub(super) fn dedupe_by_canonical(candidates: Vec<AgentCandidate>) -> Vec<AgentCandidate> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut deduped: Vec<AgentCandidate> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let canonical = candidate
            .path
            .canonicalize()
            .unwrap_or_else(|_| candidate.path.clone());
        if seen.insert(canonical) {
            deduped.push(candidate);
        } else {
            log::debug!(
                "Deduped agent candidate [{}] {}",
                candidate.source,
                mask_home_path(&candidate.path)
            );
        }
    }
    deduped
}

/// Discover all accessible SSH agent socket candidates (deduplicated).
///
/// Order: IdentityAgent → SSH_AUTH_SOCK → 1Password (3 paths) →
/// launchctl → ~/.ssh → systemd → gnome-keyring.
pub(super) async fn discover_agent_sockets(identity_agent: Option<&str>) -> Vec<AgentCandidate> {
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();

    if let Some(ia) = identity_agent {
        candidates.push(("IdentityAgent".to_string(), expand_tilde(ia)));
    }

    if let Ok(sock) = std::env::var("SSH_AUTH_SOCK") {
        if !sock.is_empty() {
            candidates.push(("SSH_AUTH_SOCK".to_string(), PathBuf::from(sock)));
        }
    }

    if let Some(home) = dirs::home_dir() {
        let op_base = home
            .join("Library")
            .join("Group Containers")
            .join("2BUA8C4S2C.com.1password");
        candidates.push((
            "1Password AppStore (t/agent.sock)".to_string(),
            op_base.join("t").join("agent.sock"),
        ));
        candidates.push((
            "1Password AppStore (agent.sock)".to_string(),
            op_base.join("agent.sock"),
        ));
        candidates.push((
            "1Password CLI".to_string(),
            home.join(".1password").join("agent.sock"),
        ));
        candidates.push((
            "~/.ssh/agent.sock".to_string(),
            home.join(".ssh").join("agent.sock"),
        ));
    }

    #[cfg(target_os = "macos")]
    {
        // 2s timeout prevents hung launchctl from blocking discovery.
        // status.success() guard prevents pushing garbage on abnormal exit.
        let launchctl_result = tokio::time::timeout(
            Duration::from_secs(2),
            tokio::process::Command::new("launchctl")
                .args(["getenv", "SSH_AUTH_SOCK"])
                .kill_on_drop(true)
                .output(),
        )
        .await;

        if let Ok(Ok(output)) = launchctl_result {
            if output.status.success() {
                let sock = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !sock.is_empty() {
                    candidates.push(("launchctl SSH_AUTH_SOCK".to_string(), PathBuf::from(sock)));
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        let uid = unsafe { libc::getuid() };
        candidates.push((
            "systemd ssh-agent.socket".to_string(),
            PathBuf::from(format!("/run/user/{}/ssh-agent.socket", uid)),
        ));
        candidates.push((
            "gnome-keyring ssh".to_string(),
            PathBuf::from(format!("/run/user/{}/keyring/ssh", uid)),
        ));
    }

    let checks = futures::future::join_all(
        candidates
            .into_iter()
            .map(|(source, path)| async move {
                let accessible = tokio::fs::metadata(&path).await.is_ok();
                accessible.then(|| AgentCandidate { source, path })
            }),
    )
    .await;

    dedupe_by_canonical(checks.into_iter().flatten().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(source: &str, path: impl Into<PathBuf>) -> AgentCandidate {
        AgentCandidate {
            source: source.to_string(),
            path: path.into(),
        }
    }

    /// Real symlink dedup: 2 candidates (real + symlink to it) → 1 result.
    #[cfg(unix)]
    #[test]
    fn test_dedupe_by_canonical_merges_symlinks() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let real = tmp_dir.path().join("real.sock");
        std::fs::write(&real, b"fake").unwrap();
        let link = tmp_dir.path().join("link.sock");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let candidates = vec![cand("real", &real), cand("symlink", &link)];
        let deduped = dedupe_by_canonical(candidates);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].source, "real");
    }

    /// canonicalize failure (broken symlink) → falls back to raw path.
    /// Two distinct broken symlinks should NOT be deduped (different raw paths).
    #[cfg(unix)]
    #[test]
    fn test_dedupe_by_canonical_handles_broken_symlink() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let broken1 = tmp_dir.path().join("broken1.sock");
        let broken2 = tmp_dir.path().join("broken2.sock");
        // Symlinks pointing to non-existent targets — canonicalize will fail
        std::os::unix::fs::symlink("/nonexistent/target1", &broken1).unwrap();
        std::os::unix::fs::symlink("/nonexistent/target2", &broken2).unwrap();

        let candidates = vec![cand("b1", &broken1), cand("b2", &broken2)];
        let deduped = dedupe_by_canonical(candidates);
        // Both kept (canonicalize failed, raw paths differ)
        assert_eq!(deduped.len(), 2);
    }

    #[test]
    fn test_dedupe_by_canonical_preserves_distinct_paths() {
        let tmp1 = tempfile::NamedTempFile::new().unwrap();
        let tmp2 = tempfile::NamedTempFile::new().unwrap();
        let candidates = vec![cand("a", tmp1.path()), cand("b", tmp2.path())];
        let deduped = dedupe_by_canonical(candidates);
        assert_eq!(deduped.len(), 2);
    }

    #[test]
    fn test_mask_home_path_masks_home_prefix() {
        let home = dirs::home_dir().unwrap();
        let path = home.join("Library").join("foo.sock");
        let masked = mask_home_path(&path);
        assert!(masked.starts_with("~/"));
        assert!(!masked.contains(home.to_str().unwrap()));
    }

    #[test]
    fn test_mask_home_path_passes_through_non_home() {
        let masked = mask_home_path(std::path::Path::new("/run/user/1000/ssh-agent.socket"));
        assert_eq!(masked, "/run/user/1000/ssh-agent.socket");
    }

    /// Real ordering test: identity_agent (accessible) must be first.
    #[tokio::test]
    async fn test_identity_agent_is_first_when_accessible() {
        // Isolate from env to make test deterministic
        unsafe { std::env::remove_var("SSH_AUTH_SOCK"); }

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let tmp_path = tmp.path().to_str().unwrap().to_string();
        let result = discover_agent_sockets(Some(&tmp_path)).await;
        assert!(!result.is_empty(), "IdentityAgent candidate must be present");
        assert_eq!(result[0].source, "IdentityAgent");
        assert_eq!(result[0].path, std::path::PathBuf::from(&tmp_path));
    }

    /// Smoke test: every returned path must exist.
    #[tokio::test]
    async fn test_discover_returns_only_existing_paths() {
        unsafe { std::env::remove_var("SSH_AUTH_SOCK"); }
        let result = discover_agent_sockets(None).await;
        for c in &result {
            assert!(c.path.exists(), "Returned path should exist: {:?}", c.path);
        }
    }

    #[tokio::test]
    async fn test_inaccessible_paths_filtered() {
        unsafe { std::env::remove_var("SSH_AUTH_SOCK"); }
        let result = discover_agent_sockets(Some("/this/path/does/not/exist/agent.sock")).await;
        for c in &result {
            assert_ne!(c.source, "IdentityAgent");
        }
    }
}

/// Discover the SSH agent socket path (free function version).
pub(super) fn discover_agent_socket_static() -> Option<String> {
    // 1. Check SSH_AUTH_SOCK environment variable
    if let Ok(sock) = std::env::var("SSH_AUTH_SOCK") {
        if !sock.is_empty() && std::path::Path::new(&sock).exists() { return Some(sock); }
    }
    // 2. macOS: check launchctl for SSH_AUTH_SOCK
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("launchctl").args(["getenv", "SSH_AUTH_SOCK"]).output() {
            let sock = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !sock.is_empty() && std::path::Path::new(&sock).exists() { return Some(sock); }
        }
    }
    let home = dirs::home_dir()?;
    // 3. 1Password Mac App Store agent socket
    #[cfg(target_os = "macos")]
    {
        let op_app_store = home.join("Library").join("Group Containers").join("2BUA8C4S2C.com.1password").join("agent.sock");
        if op_app_store.exists() { return Some(op_app_store.to_string_lossy().to_string()); }
    }
    // 4. 1Password CLI agent socket
    let op_cli = home.join(".1password").join("agent.sock");
    if op_cli.exists() { return Some(op_cli.to_string_lossy().to_string()); }
    // 5. ~/.ssh/agent.sock
    let ssh_agent = home.join(".ssh").join("agent.sock");
    if ssh_agent.exists() { return Some(ssh_agent.to_string_lossy().to_string()); }
    // 6-7. Linux system agent socket paths
    #[cfg(target_os = "linux")]
    {
        let uid = unsafe { libc::getuid() };
        let uid_str = uid.to_string();
        for p in &[format!("/run/user/{}/ssh-agent.socket", uid_str), format!("/run/user/{}/keyring/ssh", uid_str)] {
            if std::path::Path::new(p).exists() { return Some(p.clone()); }
        }
    }
    None
}
