//! SSH Agent socket 发现（静态化版本）。

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
