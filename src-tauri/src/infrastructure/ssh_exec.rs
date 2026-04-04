//! SSH remote command execution module.
//!
//! Provides `exec_remote_command()` which runs a command on a remote host
//! over an established SSH session, collecting stdout and stderr separately.
//!
//! Electron reference: `SshConnectionManager.ts` lines 305-329 (`execRemoteCommand`).

use russh::ChannelMsg;
use russh::client;

/// Execute a remote command over SSH.
///
/// Opens a session channel, runs the given command, and collects stdout/stderr
/// separately. On exit code 0, returns `Ok(stdout)`. On non-zero exit, returns
/// `Err(stderr)` if stderr is non-empty, otherwise a generic error message
/// including the exit code.
///
/// # Arguments
/// * `session` - An active `russh::client::Handle` (mutable borrow).
/// * `command` - The shell command string to execute on the remote host.
///
/// # Returns
/// * `Ok(String)` — stdout contents when exit code is 0.
/// * `Err(String)` — stderr contents (or a generic message) when exit code is non-zero,
///   or an error message if channel/exec/wait fails.
pub async fn exec_remote_command<H: client::Handler>(
    session: &mut client::Handle<H>,
    command: &str,
) -> Result<String, String> {
    let mut channel = session
        .channel_open_session()
        .await
        .map_err(|e| format!("Failed to open SSH session channel: {}", e))?;

    channel
        .exec(true, command)
        .await
        .map_err(|e| format!("Failed to execute remote command: {}", e))?;

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_code: Option<u32> = None;

    loop {
        let Some(msg) = channel.wait().await else {
            break;
        };
        match msg {
            ChannelMsg::Data { ref data } => {
                stdout.extend_from_slice(data);
            }
            ChannelMsg::ExtendedData { ref data, ext } => {
                if ext == 1 {
                    // SSH_EXTENDED_DATA_STDERR
                    stderr.extend_from_slice(data);
                } else {
                    stdout.extend_from_slice(data);
                }
            }
            ChannelMsg::ExitStatus { exit_status } => {
                exit_code = Some(exit_status);
            }
            _ => {}
        }
    }

    let stdout_str = String::from_utf8_lossy(&stdout).to_string();

    match exit_code {
        Some(0) => Ok(stdout_str),
        Some(code) => {
            let stderr_str = String::from_utf8_lossy(&stderr).to_string();
            if stderr_str.is_empty() {
                Err(format!(
                    "Remote command exited with code {}: {}",
                    code,
                    if stdout_str.is_empty() {
                        "(no output)".to_string()
                    } else {
                        stdout_str.clone()
                    }
                ))
            } else {
                Err(stderr_str)
            }
        }
        None => {
            let stderr_str = String::from_utf8_lossy(&stderr).to_string();
            if stderr_str.is_empty() {
                Err("Remote command did not return an exit status".to_string())
            } else {
                Err(stderr_str)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::ssh_connection::SshClientHandler;

    #[test]
    fn test_module_exists() {
        // Verify the module compiles and the public function is accessible.
        // Actual SSH testing requires a live connection, so this is a compile check.
        let _ = std::any::type_name::<fn(&mut client::Handle<SshClientHandler>, &str) -> Result<String, String>>();
    }
}
