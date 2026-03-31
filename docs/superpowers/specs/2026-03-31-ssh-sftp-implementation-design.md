# SSH/SFTP Phase 2 Implementation Design

> **Date**: 2026-03-31
>
> **Goal**: Replace Phase 1 stubs with fully functional SSH/SFTP, achieving feature parity with the Electron app ([claude-devtools](https://github.com/matt1398/claude-devtools)).
>
> **Electron reference**: `SshConnectionManager.ts` (544 lines), `SshFileSystemProvider.ts` (274 lines)

## Dependencies

```toml
# Cargo.toml — new dependency
russh-sftp = "2.0"   # SFTP v3 client

# Existing (already declared)
russh = "0.46"
russh-keys = "0.46"
ssh_config = "0.1"
```

`russh` has no built-in SFTP module. `russh-sftp` is the official companion crate. Note: `russh-sftp` lists `russh` only as a **dev dependency** (for examples/tests), not a runtime dependency — it accepts any `AsyncRead + AsyncWrite + Unpin + Send` stream, so there is no version conflict with our `russh = "0.46"`. Integration pattern:

```
russh channel → request_subsystem("sftp") → into_stream() → SftpSession::new(stream)
```

## Module Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `ssh_connection_manager.rs` | **Rewrite** | Connection lifecycle, SFTP channel, remote exec, provider creation |
| `ssh_fs_provider.rs` | **Rewrite** | FsProvider trait over SFTP, retry logic, error classification |
| `ssh_auth.rs` | **New** | Authentication methods (password, key, agent, auto fallback) |
| `ssh_exec.rs` | **New** | Remote command execution over SSH channel |
| `Cargo.toml` | **Edit** | Add `russh-sftp = "2.0"` |

## Architecture

### Async-to-Sync Bridge

`FsProvider` trait is fully synchronous (`fn`, not `async fn`). All consumers (ProjectScanner, SessionSearcher, FileWatcher) use sync calls. Bridge via `tokio::runtime::Handle::block_on()`:

```rust
fn read_file(&self, path: &Path) -> Result<String, String> {
    let path = path.to_path_buf();
    self.handle.block_on(async {
        let data = self.sftp.read(&path).await
            .map_err(|e| format!("SFTP read error: {}", e))?;
        String::from_utf8(data).map_err(|e| format!("UTF-8 error: {}", e))
    })
}
```

### SftpSession Thread Safety

`russh-sftp::client::SftpSession` methods take `&self` (not `&mut self`) and the struct is `Send + Sync`. This aligns well with `FsProvider: Send + Sync + Debug`. No Mutex wrapper needed:

```rust
pub struct SshFsProvider {
    sftp: Arc<SftpSession>,   // Shared reference, &self methods are safe
    handle: tokio::runtime::Handle,
}
```

Note: `SftpSession` must implement `Debug` (required by `FsProvider` trait). If the auto-derived impl is not available due to private fields, implement `Debug` manually:
```rust
impl std::fmt::Debug for SshFsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshFsProvider").finish_non_exhaustive()
    }
}
```

## SshConnectionManager Rewrite

### russh Client Handler

Empty implementation satisfying `russh::client::Handler` trait. Accepts all server keys (matching Electron's ssh2 default behavior of no strict host key verification):

```rust
struct SshClientHandler;

#[async_trait]
impl russh::client::Handler for SshClientHandler {
    type Error = SshError;
    async fn check_server_key(&mut self, key: &ssh_key::PublicKey) -> Result<bool, Self::Error> {
        Ok(true)  // TODO: future known_hosts verification
    }
}
```

### Internal State

```rust
struct SshConnection {
    config: SshConnectionConfig,
    status: SshConnectionStatus,
    remote_projects_path: Option<String>,
    session: russh::client::Handle<SshClientHandler>,  // SSH session
    sftp: SshFsProvider,                                 // SFTP provider (implements FsProvider)
}
```

### connect() Flow

Aligns with Electron `SshConnectionManager.connect()` (lines 93-137):

```
1. Auto-disconnect existing connection
2. Emit Connecting status
3. Merge with SSH config (merge_with_ssh_config)
4. Validate host/username are non-empty
5. Resolve username: config.username || ssh_config.user || $USER || "root"
   (Electron: config.username || sshConfig?.user || os.userInfo().username)
6. russh::client::connect(Arc<Config>, (host, port), handler)  [10s timeout]
7. Authenticate (see ssh_auth.rs)
8. channel_open_session() + request_subsystem("sftp")
9. SftpSession::new(channel.into_stream())
10. Create SshFsProvider(sftp_session, runtime_handle)
11. Store in SshConnection
12. exec_remote_command("printf %s \"$HOME\"") to get remote $HOME
13. resolve_remote_projects_path(home, username)
14. Spawn background disconnect monitor task
15. Emit Connected status
```

### disconnect() Flow

Aligns with Electron `SshConnectionManager.disconnect()` (lines 187-196):

```
1. Cancel disconnect monitor task
2. SftpSession cleanup (sftp.close() or let Drop handle it)
3. session.disconnect(Disconnect::ByApplication, "", "")
4. Set connection = None
5. Emit Disconnected status
```

### test() Flow

Aligns with Electron `testConnection()` (lines 148-170):

```
1. Validate host/username
2. Create temporary russh session (separate from main connection)
3. Connect + authenticate [10s timeout]
4. Open SFTP subsystem to verify full access
5. Disconnect temporary session
6. Return SshTestResult { success: true, error: None }
On any error: return { success: false, error: message }
```

### get_provider() Flow

Aligns with Electron `getProvider()` (line 63):

```rust
pub async fn get_provider() -> Option<Arc<dyn FsProvider>> {
    let conn = self.connection.read().await;
    conn.as_ref().map(|c| Arc::new(c.sftp.clone()) as Arc<dyn FsProvider>)
}
```

No longer returns `None` when connected. Commands/routes get the real SFTP provider.

### Passive Disconnect Detection

Aligns with Electron's event handlers (lines 113-122):

```typescript
// Electron
client.on('end', () => { this.handleDisconnect(); });
client.on('close', () => { this.handleDisconnect(); });
client.on('error', (err) => { this.lastError = err.message; this.setState('error'); });
```

Implementation: spawn a background tokio task after successful connect that monitors the SSH session for disconnect. In russh, unlike ssh2's event-based `client.on('end')`, there is no callback mechanism. The approach is to periodically check the session state or attempt a no-op channel operation that fails when disconnected:

```rust
// Option A: Monitor via periodic session check
tokio::spawn(async move {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        // Try to open a channel — fails if session is disconnected
        if session.channel_open_session().await.is_err() {
            // Connection lost — trigger handle_disconnect()
            break;
        }
    }
});

// Option B: Store session in Arc, check Handle::is_connected() if available
// (verify russh 0.46 API during implementation)
```

When detected:

```
1. Switch provider back to local (via callback or direct ContextManager access)
2. Clear remoteProjectsPath
3. Emit Disconnected status
4. Update internal state
```

### merge_with_ssh_config() Enhancement

Add OS username fallback (matching Electron line 131):

```rust
// Existing: host, port, IdentityFile promotion
// NEW: username fallback
if merged.username.is_empty() {
    if let Some(user) = entry.user {
        merged.username = user;
    } else {
        // Electron: os.userInfo().username
        merged.username = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "root".to_string());
    }
}
```

### resolve_remote_projects_path()

Aligns with Electron `resolveRemoteProjectsPath()` (lines 222-241):

```rust
async fn resolve_remote_projects_path(
    sftp: &SftpSession,
    username: &str,
) -> String {
    // 1. Query remote $HOME via exec_remote_command
    let remote_home = exec_remote_command("printf %s \"$HOME\"").await.ok()
        .and_then(|h| {
            let trimmed = h.trim().to_string();
            if trimmed.starts_with('/') { Some(trimmed) } else { None }
        });

    // 2. Build candidate paths (matching Electron exactly)
    let mut candidates = Vec::new();
    if let Some(ref home) = remote_home {
        candidates.push(format!("{}/.claude/projects", home));
    }
    candidates.push(format!("/home/{}/.claude/projects", username));
    candidates.push(format!("/Users/{}/.claude/projects", username));
    candidates.push("/root/.claude/projects".to_string());

    // Deduplicate while preserving order
    let mut seen = HashSet::new();
    candidates.retain(|c| seen.insert(c.clone()));

    // 3. Test each with SFTP exists
    for candidate in &candidates {
        if sftp.try_exists(candidate).await.unwrap_or(false) {
            return candidate.clone();
        }
    }

    // 4. Fallback
    remote_home
        .map(|h| format!("{}/.claude/projects", h))
        .unwrap_or_else(|| format!("/home/{}/.claude/projects", username))
}
```

## ssh_auth.rs — Authentication Module

Aligns with Electron `buildConnectConfig()` auth section (lines 147-183) and `resolveAutoAuth()` (lines 267-303).

### Password Auth

```rust
// Electron: connectConfig.password = config.password
session.authenticate_password(username, password).await
```

### PrivateKey Auth

```rust
// Electron: reads key file, sets connectConfig.privateKey
// Default path: ~/.ssh/id_rsa
let key_path = config.private_key_path.as_deref()
    .unwrap_or("~/.ssh/id_rsa");
let expanded = expand_tilde(key_path);
let secret_key = russh_keys::load_secret_key(&expanded, None)
    .map_err(|e| format!("Cannot read private key at {}: {}", expanded, e))?;
session.authenticate_publickey(username, Arc::new(secret_key)).await
```

Note: `passphrase` is `None` since the UI has no passphrase input. Encrypted keys will fail with a clear error message (matching Electron behavior where encrypted keys without passphrase handling just fail).

### Agent Auth

```rust
// Electron: discovers agent socket, sets connectConfig.agent
let socket = SshConnectionManager::discover_agent_socket()
    .ok_or_else(|| "SSH agent socket not found".to_string())?;
let mut agent = russh_keys::agent::client::AgentClient::connect_env()
    .await
    .map_err(|e| format!("Failed to connect to SSH agent: {}", e))?;
let identities = agent.request_identities().await
    .map_err(|e| format!("Failed to list agent identities: {}", e))?;
// Use first identity for authentication
let identity = identities.into_iter().next()
    .ok_or_else(|| "No identities found in SSH agent".to_string())?;
session.authenticate_publickey_with_agent(username, agent, identity).await
```

### Auto Auth — Fallback Chain

Aligns exactly with Electron `resolveAutoAuth()` (lines 267-303):

```
Step 1: If SSH config has IdentityFile
  → Re-resolve host via configParser.resolveHost(sshConfig.alias)
  → If resolved, try default key paths:
    → ~/.ssh/id_ed25519 (read file, load_secret_key, authenticate)
    → ~/.ssh/id_rsa (read file, load_secret_key, authenticate)
  → If either succeeds, return Ok

Step 2: Try SSH Agent
  → discover_agent_socket()
  → connect to agent, get identities, authenticate with first identity
  → If succeeds, return Ok

Step 3: Try default key files
  → ~/.ssh/id_ed25519
  → ~/.ssh/id_rsa
  → ~/.ssh/id_ecdsa
  → If any succeeds, return Ok

Step 4: All failed → return Err("No supported authentication method succeeded")
```

### Timeout

All auth operations wrapped in `tokio::time::timeout(10s)`:

```rust
const AUTH_TIMEOUT_SECS: u64 = 10;
tokio::time::timeout(
    Duration::from_secs(AUTH_TIMEOUT_SECS),
    session.authenticate_password(username, password)
).await.map_err(|_| "Authentication timed out (10s)".to_string())?
```

## ssh_exec.rs — Remote Command Execution

Aligns with Electron `execRemoteCommand()` (lines 305-329).

```rust
pub async fn exec_remote_command(
    session: &Handle<SshClientHandler>,
    command: &str,
) -> Result<String, String> {
    let channel = session.channel_open_session().await
        .map_err(|e| format!("Failed to open exec channel: {}", e))?;
    channel.exec(true, command).await
        .map_err(|e| format!("Failed to exec command: {}", e))?;

    // russh Channel implements AsyncRead for stdout data
    // Use channel.stderr() for stderr stream (if available)
    let mut stdout = Vec::new();
    let mut stdout_reader = channel.make_reader();  // consumes channel data stream
    use tokio::io::AsyncReadExt;
    stdout_reader.read_to_end(&mut stdout).await
        .map_err(|e| format!("Failed to read command output: {}", e))?;

    // Wait for exit status via channel exit_status()
    // russh sends exit-status as a channel request, not via stream EOF
    let exit_code = channel.exit_status().await
        .map_err(|e| format!("Failed to get exit status: {}", e))?;

    match exit_code {
        Some(0) => Ok(String::from_utf8_lossy(&stdout).to_string()),
        Some(code) => {
            let stderr = String::from_utf8_lossy(&stdout).trim().to_string();
            let msg = if stderr.is_empty() {
                format!("Remote command failed with exit code {}", code)
            } else {
                stderr
            };
            Err(msg)
        }
        None => Err("Remote command failed with unknown exit code".to_string()),
    }
}
```

Note: russh's channel API differs from ssh2's event-based `stream.on('data', ...)`. In russh, `channel.make_reader()` returns an `AsyncRead` impl that reads from the channel's data stream. The exit status arrives as a separate `channel.exit_status()` future. The exact API may need adjustment based on russh 0.46's actual `Handle` and `Channel` method signatures — consult russh docs during implementation.

Usage:
- `resolve_remote_home_directory()`: `exec_remote_command('printf %s "$HOME"')`
- Future: remote git operations, etc.

## SshFsProvider Rewrite

### Structure

```rust
pub struct SshFsProvider {
    sftp: tokio::sync::Mutex<SftpSession>,
    handle: tokio::runtime::Handle,
}
```

Implements `FsProvider` trait. Each sync method bridges to async via `handle.block_on()`.

### Error Classification

Aligns with Electron `classifySftpError()` (lines 237-257):

```rust
#[derive(Debug, PartialEq)]
enum SftpErrorKind {
    NotFound,    // SFTP status code 2 / SSH_FX_NO_SUCH_FILE
    Transient,   // SFTP status code 4 / SSH_FX_FAILURE + connection errors
    Permanent,   // everything else
}

fn classify_sftp_error(error: &russh_sftp::SftpError) -> SftpErrorKind {
    // Map russh_sftp error to Electron-compatible classification
    // russh-sftp returns SftpError with status codes
    // code 2 → NotFound
    // code 4 → Transient
    // Connection errors (EAGAIN, ECONNRESET, etc.) → Transient
    // Everything else → Permanent
}
```

### Retry Logic

Aligns with Electron (lines 24-40):

```rust
const MAX_RETRIES: u32 = 3;
const RETRY_BASE_DELAY_MS: u64 = 75;

// Pattern (applied to read_file, stat, read_dir only):
for attempt in 1..=MAX_RETRIES {
    match sftp_operation().await {
        Ok(result) => return Ok(result),
        Err(e) if classify_sftp_error(&e) == Transient && attempt < MAX_RETRIES => {
            tokio::time::sleep(Duration::from_millis(RETRY_BASE_DELAY_MS * attempt as u64)).await;
            continue;
        }
        Err(e) => return Err(e.to_string()),
    }
}
```

Linear backoff: attempt 1 fail → sleep 75ms → attempt 2 → fail → sleep 150ms → attempt 3 → fail → throw.

### Method Implementations

#### exists()

Aligns with Electron (lines 66-85):

```rust
fn exists(&self, path: &Path) -> Result<bool, String> {
    self.handle.block_on(async {
        match self.sftp.try_exists(path).await {
            Ok(true) => Ok(true),
            Ok(false) => Ok(false),
            Err(e) => match classify_sftp_error(&e) {
                NotFound => Ok(false),
                Transient => Ok(true),    // Conservative: avoid false negatives
                Permanent => Ok(false),
            },
        }
    })
}
```

#### read_file()

Aligns with Electron (lines 87-108):

```rust
fn read_file(&self, path: &Path) -> Result<String, String> {
    let path = path.to_path_buf();
    self.handle.block_on(async {
        let mut last_error = None;
        for attempt in 1..=MAX_RETRIES {
            match self.sftp.read(&path).await {
                Ok(data) => return String::from_utf8(data)
                    .map_err(|e| format!("UTF-8 error: {}", e)),
                Err(e) => {
                    last_error = Some(e);
                    if classify_sftp_error(&last_error.as_ref().unwrap()) == Transient
                        && attempt < MAX_RETRIES
                    {
                        tokio::time::sleep(Duration::from_millis(RETRY_BASE_DELAY_MS * attempt as u64)).await;
                        continue;
                    }
                    break;
                }
            }
        }
        Err(last_error.unwrap().to_string())
    })
}
```

#### read_file_head()

No direct Electron equivalent (Electron uses createReadStream with byte offset). Implementation using russh-sftp's `open()` to get a `File` handle, then `AsyncReadExt` to read content and split into lines:

```rust
fn read_file_head(&self, path: &Path, max_lines: usize) -> Result<String, String> {
    let path = path.to_path_buf();
    self.handle.block_on(async {
        // With retry for transient errors (same pattern as read_file)
        let data = self.sftp.read(&path).await
            .map_err(|e| format!("SFTP read error: {}", e))?;
        let text = String::from_utf8(data)
            .map_err(|e| format!("UTF-8 error: {}", e))?;
        let lines: Vec<&str> = text.lines().take(max_lines).collect();
        Ok(lines.join("\n"))
    })
}
```

Note: This reads the full file into memory then truncates. For large files, `sftp.open()` returns a `File` handle implementing `AsyncRead`, which could be used for streaming reads. The full-read approach is acceptable since session files are typically <50MB and `read_file_head` is used for metadata preview.

#### stat()

Aligns with Electron (lines 119-153):

```rust
fn stat(&self, path: &Path) -> Result<FsStatResult, String> {
    // With retry for transient errors
    // Convert russh-sftp Metadata → FsStatResult:
    //   size: metadata.len()
    //   mtime_ms: metadata.modified() as millis
    //   birthtime_ms: metadata.modified() as millis (SFTP has no birthtime, use mtime fallback)
    //   is_file: from mode bitmask (S_IFREG)
    //   is_directory: from mode bitmask (S_IFDIR)
}
```

Electron uses POSIX mode bitmask: `S_IFMT=0o170000`, `S_IFREG=0o100000`, `S_IFDIR=0o040000`. Same approach in Rust.

#### read_dir()

Aligns with Electron (lines 155-190):

```rust
fn read_dir(&self, path: &Path) -> Result<Vec<FsDirent>, String> {
    let path = path.to_path_buf();
    self.handle.block_on(async {
        // With retry for transient errors (same pattern as read_file)
        let entries = self.sftp.read_dir(&path).await
            .map_err(|e| format!("SFTP readdir error: {}", e))?;
        // russh-sftp returns ReadDir which iterates DirEntry items
        // Each DirEntry has file_name() and metadata()
        // Convert to FsDirent: name, is_file (from mode bitmask), is_directory,
        //   size, mtime_ms, birthtime_ms — all from SFTP attrs inline
        // No additional stat calls needed (unlike LocalFsProvider)
        let result: Vec<FsDirent> = entries.iter().map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let metadata = entry.metadata();
            let mode = metadata.file_type().as_raw_mode();
            let is_file = (mode & 0o170000) == 0o100000; // S_IFREG
            let is_directory = (mode & 0o170000) == 0o040000; // S_IFDIR
            FsDirent {
                name,
                is_file,
                is_directory,
                size: Some(metadata.len()),
                mtime_ms: Some(metadata.modified().as_millis()),
                birthtime_ms: Some(metadata.modified().as_millis()), // mtime fallback
            }
        }).collect();
        Ok(result)
    })
}
```

### Drop

```rust
impl Drop for SshFsProvider {
    fn drop(&mut self) {
        // SftpSession drop closes the SFTP protocol channel automatically
        // No explicit cleanup needed (matching Electron's sftp.end() in dispose())
    }
}
```

### Clone

`SshFsProvider` must be `Clone` so it can be wrapped in `Arc` for `get_provider()`:

```rust
// SshFsProvider wraps Arc<SftpSession> internally
// Arc<SftpSession> is Clone, so SshFsProvider derives Clone automatically
```

## Changes to Existing Files

### commands/ssh.rs and http/routes/ssh.rs

Remove manual `SshFsProvider::new(host, port, "ssh")` placeholder creation. Instead:

```rust
// Before (Phase 1):
let fs_provider = match mgr.get_provider().await {
    Some(provider) => provider,
    None => Arc::new(SshFsProvider::new(host, port, "ssh")),  // placeholder
};

// After (Phase 2):
let fs_provider = mgr.get_provider().await
    .ok_or_else(|| error_json("SSH provider not available after connect"))?;
```

## Testing Strategy

| Module | Test Type | Scope |
|--------|-----------|-------|
| `ssh_auth.rs` | Unit tests | Key file resolution logic, Auto fallback chain (mock fs) |
| `ssh_exec.rs` | None | Requires real SSH connection |
| `ssh_fs_provider.rs` | Unit tests | Error classification, retry counting, Metadata→FsStatResult conversion |
| `ssh_connection_manager.rs` | Unit tests | State machine transitions, config merging, discover_agent_socket, resolve_remote_home |

No integration tests requiring a real SSH server. Manual verification via `pnpm tauri dev` connecting to a remote host.

## Issue Resolution Mapping

This implementation resolves the following issues from check-04.md:

| Issue | Description | Resolution |
|-------|-------------|------------|
| #1 | SFTP Not Implemented | SshFsProvider fully implemented over russh-sftp |
| #2 | No Real SSH Connection | russh connect + auth + SFTP channel |
| #3 | Agent Socket Discovery | Already fixed (previous iteration) |
| #4 | Auto Auth No Fallback | Full fallback chain in ssh_auth.rs |
| #5 | Remote Path Hardcoded | exec_remote_command + candidate fallback |
| #7 | No Remote Command Exec | ssh_exec.rs module |
| #8 | No SFTP Retry Logic | 3 retries, 75ms linear backoff |
| #9 | No SFTP Error Classification | NotFound/Transient/Permanent classification |
| #13 | read_dir Inconsistency | Already fixed (previous iteration) |

Remaining issues after this implementation:
- #6 (Include directives) — Already fixed
- #10 (createReadStream) — N/A (tech stack difference)
- #11 (LocalFsProvider concurrent stat) — Already addressed
- #12 (HTTP stubs) — Already fixed
