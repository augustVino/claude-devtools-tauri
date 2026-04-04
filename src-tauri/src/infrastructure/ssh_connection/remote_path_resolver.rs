//! 远程项目路径解析（静态化版本）。

use russh::client;
use crate::infrastructure::fs_provider::FsProvider;
use crate::infrastructure::ssh_exec::exec_remote_command;
use crate::infrastructure::ssh_fs_provider::SshFsProvider;
use crate::types::ssh::SshConnectionConfig;

/// Resolve the remote projects path (free function version).
pub(super) async fn resolve_remote_projects_path_static(
    session: &mut client::Handle<super::SshClientHandler>,
    username: &str,
    fs_provider: &SshFsProvider,
) -> String {
    let home = match exec_remote_command(session, "printf %s \"$HOME\"").await {
        Ok(h) if !h.trim().is_empty() => h.trim().to_string(),
        _ => format!("/home/{}", username),
    };
    let mut candidates: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for candidate in &[
        format!("{}/.claude/projects", home),
        format!("/home/{}/.claude/projects", username),
        format!("/Users/{}/.claude/projects", username),
        "/root/.claude/projects".to_string(),
    ] {
        if seen.insert(candidate.clone()) { candidates.push(candidate.clone()); }
    }
    for candidate in &candidates {
        match fs_provider.exists_async(candidate).await {
            Ok(true) => { log::info!("Remote projects path resolved to: {}", candidate); return candidate.clone(); }
            _ => continue,
        }
    }
    let fallback = format!("{}/.claude/projects", home);
    log::info!("No existing remote projects path found, using fallback: {}", fallback);
    fallback
}
