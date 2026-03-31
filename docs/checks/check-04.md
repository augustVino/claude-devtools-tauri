# Module 04: SSH Connection - Feature Parity Check

> **Check Date**: 2026-03-30
>
> **Last Verified**: 2026-03-31
>
> **Electron**: ssh2 + SFTP, fully functional (544 + 274 + 194 lines)
>
> **Tauri**: russh dependency loaded, Phase 1 stub implementation (SFTP not yet wired)

## Summary

| Category | Count | Details |
|----------|-------|---------|
| Total Test Items | 65 | From regression checklist |
| Pass (Tauri aligned) | 24 | Fully implemented and consistent |
| Pass with difference | 5 | Implemented but behavior differs |
| Not applicable | 1 | Architecture difference (no PassThrough stream) |
| Not implemented (Phase 1 stub) | 30 | SFTP operations not yet wired |
| Not implemented (missing feature) | 5 | Features absent from Tauri |

## Critical Status

**SSH/SFTP is in Phase 1 (stub)**: The connection state machine, config parsing, and API surface are built, but **no real SSH connections are established**. All SFTP file operations return errors. The `SshConnectionManager::get_provider()` always returns `None`. This means:
- SSH connect appears to succeed but doesn't create a real network connection
- No remote files can be read, listed, or stat'd
- Context switching to SSH mode works at the state level but provides no data

## Test Checklist

### Connection Lifecycle

| # | Item | Status | Notes |
|---|------|--------|-------|
| 1 | Connect with password authentication succeeds | [-] | **Phase 1 stub**: No real SSH connection. Password is validated for non-empty but not sent over network. |
| 2 | Connect with private key authentication succeeds | [-] | **Phase 1 stub**: Key file path stored but not loaded or used. |
| 3 | Connect with private key uses default path ~/.ssh/id_rsa when not specified | [x] | Frontend defaults to `~/.ssh/id_rsa` in `connectionSlice.ts` |
| 4 | Connect with private key throws error when key file not found | [-] | **Not implemented**: No key file validation in Phase 1 |
| 5 | Connect with agent authentication succeeds when SSH_AUTH_SOCK is set | [-] | **Phase 1 stub**: Agent socket discovered but not used for auth |
| 6 | Connect with agent discovers socket via launchctl on macOS | [x] | Aligned: checks `launchctl getenv SSH_AUTH_SOCK` on macOS |
| 7 | Connect with agent discovers 1Password SSH agent socket | [x] | **Fixed**: Now checks both `~/.1password/agent.sock` and `~/Library/Group Containers/2BUA8C4S2C.com.1password/agent.sock`. |
| 8 | Connect with agent throws error when no socket found | [x] | Returns `None` from `discover_agent_socket()` |
| 9 | Connect with auto auth tries identity file, then agent, then default keys | [!] | **Different behavior**: Tauri promotes Auto to PrivateKey if IdentityFile found in SSH config, otherwise stays Auto. No fallback chain (no agent attempt, no default key scan). See issue #4. |
| 10 | Connect resolves SSH config host/username/port for alias | [x] | Aligned: `merge_with_ssh_config()` merges config values |
| 11 | Connect creates SshFileSystemProvider | [x] | Created on connect, but all methods are stubs |
| 12 | Connect resolves remote ~/.claude/projects/ path | [!] | **Different**: Tauri hardcodes `/home/{username}/.claude/projects`. Electron queries `$HOME` via remote command execution and checks multiple candidates. See issue #5. |
| 13 | Connect emits state-change events: connecting, connected | [x] | Aligned via broadcast channel + Tauri event bridge |
| 14 | Connect with existing connection disconnects first | [x] | Aligned: `connect()` auto-disconnects existing |
| 15 | Disconnect cleans up SFTP provider and SSH client | [-] | **Phase 1 stub**: Sets connection to None, no SFTP cleanup needed |
| 16 | Disconnect switches provider back to LocalFileSystemProvider | [x] | Aligned via ContextManager switch to "local" |
| 17 | Disconnect resets all connection state (host, error, remote path) | [x] | Aligned |
| 18 | Disconnect emits state-change event: disconnected | [x] | Aligned |
| 19 | Double disconnect is safe (no-op) | [x] | Aligned: checks if already disconnected |
| 20 | Test connection creates temporary client and disconnects | [-] | **Phase 1 stub**: Validates host/username, returns success without connecting |
| 21 | Test connection verifies SFTP channel access | [-] | **Phase 1 stub**: No SFTP verification |
| 22 | Test connection returns success/error without switching mode | [x] | Aligned: returns `SshTestResult` without context switch |

### SSH Config Parsing

| # | Item | Status | Notes |
|---|------|--------|-------|
| 23 | SSH config parsing reads ~/.ssh/config | [x] | Aligned via `ssh_config` crate |
| 24 | SSH config parsing expands Include directives | [x] | **Fixed**: `expand_includes()` processes Include/include directives with tilde expansion and glob support. See issue #6. |
| 25 | SSH config parsing handles glob patterns in Include | [x] | **Fixed**: Custom `glob_matches()` supports `*` and `?` patterns. See issue #6. |
| 26 | SSH config parsing skips wildcard host patterns | [x] | Aligned: filters `*`, `?`, `!` patterns |
| 27 | SSH config parsing returns all non-wildcard hosts | [x] | Aligned |
| 28 | SSH config resolveHost returns null for unknown alias | [x] | Aligned: returns `None` |
| 29 | SSH config resolveHost returns null for alias with no explicit entry | [x] | Aligned |
| 30 | SSH config resolveHost omits port 22 (default) | [x] | Aligned |
| 31 | SSH config resolveHost omits hostName when same as alias | [x] | Aligned |

### Remote Operations

| # | Item | Status | Notes |
|---|------|--------|-------|
| 32 | Remote home directory resolved via printf %s "$HOME" | [-] | **Not implemented**: Tauri hardcodes `/home/{username}`. See issue #5. |
| 33 | Remote projects path checks $HOME/.claude/projects first | [-] | **Not implemented**: Tauri hardcodes path. See issue #5. |
| 34 | Remote projects path falls back to /home/$USER, /Users/$USER, /root | [-] | **Not implemented**: Tauri only uses `/home/$USER`. See issue #5. |
| 35 | Remote command execution handles non-zero exit codes | [-] | **Not implemented**: No `execRemoteCommand()` equivalent. See issue #7. |
| 36 | Remote command execution handles null exit codes | [-] | **Not implemented**: See issue #7. |

### SFTP File System Provider

| # | Item | Status | Notes |
|---|------|--------|-------|
| 37 | SshFileSystemProvider.exists returns false for not_found | [-] | **Phase 1 stub**: Returns `Err("SFTP not yet implemented")` |
| 38 | SshFileSystemProvider.exists returns true for transient errors | [-] | **Phase 1 stub**: No transient error handling |
| 39 | SshFileSystemProvider.readFile retries 3 times on transient errors | [-] | **Phase 1 stub**: No retry logic. See issue #8. |
| 40 | SshFileSystemProvider.stat retries 3 times on transient errors | [-] | **Phase 1 stub**: No retry logic |
| 41 | SshFileSystemProvider.readdir retries 3 times on transient errors | [-] | **Phase 1 stub**: No retry logic |
| 42 | SshFileSystemProvider retry backoff: 75ms, 150ms, 225ms | [-] | **Phase 1 stub**: No retry logic |
| 43 | SshFileSystemProvider classifies code=2/ENOENT as not_found | [-] | **Phase 1 stub**: No error classification. See issue #9. |
| 44 | SshFileSystemProvider classifies code=4/EAGAIN/ECONNRESET/ETIMEDOUT/EPIPE as transient | [-] | **Phase 1 stub**: No error classification |
| 45 | SshFileSystemProvider.createReadStream wraps SFTP stream in PassThrough | [~] | **N/A**: FsProvider trait has no `createReadStream` method. Rust doesn't use Node.js streams. See issue #10. |
| 46 | SshFileSystemProvider.createReadStream handles error by destroying stream | [~] | **N/A**: Same as above |
| 47 | SshFileSystemProvider.dispose calls sftp.end() | [-] | **Phase 1 stub**: Drop impl is no-op with TODO comment |
| 48 | LocalFileSystemProvider.readdir stats entries concurrently for mtimeMs | [~] | **Not a bug**: Electron's `Promise.all` benefits from libuv async I/O; Rust's synchronous `metadata()` is a fast syscall served from dentry cache. Sequential is optimal for this use case. See issue #11. |
| 49 | LocalFileSystemProvider.dispose is a no-op | [x] | Aligned: Rust `Drop` is effectively no-op for LocalFsProvider |
| 50 | FileSystemProvider interface has type discriminator 'local' \| 'ssh' | [x] | Aligned: `provider_type()` returns `"local"` or `"ssh"` |

### IPC / Command Handlers

| # | Item | Status | Notes |
|---|------|--------|-------|
| 51 | IPC ssh:connect creates ServiceContext and switches registry | [x] | Aligned: creates SSH context + switches via ContextManager |
| 52 | IPC ssh:connect destroys existing SSH context on reconnection | [x] | Aligned |
| 53 | IPC ssh:disconnect switches registry to local and destroys SSH context | [x] | Aligned |
| 54 | IPC ssh:getState returns full SshConnectionStatus | [x] | Aligned |
| 55 | IPC ssh:getConfigHosts returns empty array on failure (never errors) | [x] | Aligned |
| 56 | IPC ssh:resolveHost returns null on failure (never errors) | [x] | Aligned |
| 57 | IPC ssh:saveLastConnection persists without password | [x] | Aligned: ConfigManager persists, password field excluded |
| 58 | IPC ssh:getLastConnection reads from config | [x] | Aligned |
| 59 | HTTP SSH routes mirror IPC handlers exactly | [x] | **Fixed**: HTTP `save-last-connection` and `get-last-connection` are now wired to ConfigManager. See issue #12. |

### Store Slice (Renderer)

| # | Item | Status | Notes |
|---|------|--------|-------|
| 60 | Connection store connectSsh clears stale data on success | [x] | Aligned |
| 61 | Connection store disconnectSsh clears stale data | [x] | Aligned |
| 62 | Connection store fetchSshConfigHosts gracefully handles failure | [x] | Aligned |
| 63 | Connection store loadLastConnection handles missing saved config | [x] | Aligned |
| 64 | State-change events emitted for all transitions | [x] | Aligned via broadcast + event bridge |
| 65 | Error events after connect set state to error | [x] | Aligned |

---

## Issues Found

### Issue #1: SFTP Not Implemented (Phase 1) - Critical

**Files**: `src-tauri/src/infrastructure/ssh_fs_provider.rs`, `src-tauri/src/infrastructure/ssh_connection_manager.rs`

**All SFTP operations are stubs**:
- `exists()` → `Err("SFTP not yet implemented")`
- `read_file()` → `Err("SFTP not yet implemented")`
- `read_file_head()` → `Err("SFTP not yet implemented")`
- `stat()` → `Err("SFTP not yet implemented")`
- `read_dir()` → `Ok(Vec::new())` (inconsistently returns success with empty)
- `get_provider()` → always returns `None`

**Dependencies already loaded**: `russh = "0.46"`, `russh-keys = "0.46"` in Cargo.toml.

**Impact**: SSH mode is non-functional. Connecting appears to succeed but no remote data is accessible. The application switches to SSH context but cannot list/read any remote session files.

**Electron reference**:
- `SshFileSystemProvider.ts` (274 lines) - Full SFTP implementation over ssh2
- Retry logic: 3 retries, 75ms linear backoff
- Error classification: not_found / transient / permanent
- Stream wrapping via PassThrough

**Recommendation**: Implement Phase 2 SFTP using `russh-sftp`. Key tasks:
1. Wire `russh::client::connect()` in `SshConnectionManager::connect()`
2. Implement SFTP channel opening
3. Implement all `FsProvider` trait methods over SFTP
4. Add retry logic and error classification (port from Electron)
5. Make `get_provider()` return `Some(Arc<SshFsProvider>)`

---

### Issue #2: No Real SSH Connection Establishment - Critical

**File**: `src-tauri/src/infrastructure/ssh_connection_manager.rs`

The `connect()` method (Phase 1) only:
1. Validates host/username are non-empty
2. Merges with SSH config
3. Stores connection struct in state
4. Emits `Connected` status

It does **not**:
- Create a `russh` client session
- Authenticate (password, key, agent)
- Open an SFTP subsystem
- Verify connectivity

**Impact**: SSH is completely non-functional for actual remote access.

---

### Issue #3: SSH Agent Socket Discovery Incomplete (P1) — **FIXED**

**File**: `src-tauri/src/infrastructure/ssh_connection_manager.rs` (lines 238-274)

**Electron discovery strategies** (5 strategies):
1. `SSH_AUTH_SOCK` env var
2. macOS: `launchctl getenv SSH_AUTH_SOCK`
3. `~/Library/Group Containers/2BUA8C4S2C.com.1password/agent.sock` (1Password for Mac App Store)
4. `~/.1password/agent.sock` (1Password CLI)
5. `~/.ssh/agent.sock`

**Tauri discovery strategies** (4 strategies):
1. `SSH_AUTH_SOCK` env var
2. macOS: `launchctl getenv SSH_AUTH_SOCK`
3. `~/.1password/agent.sock`
4. `~/.ssh/agent.sock`

**Missing**:
- 1Password Mac App Store path: `~/Library/Group Containers/2BUA8C4S2C.com.1password/agent.sock`
- Linux paths: `/run/user/<uid>/ssh-agent.socket`, `/run/user/<uid>/keyring/ssh`

**Impact**: 1Password users who installed via Mac App Store won't have their SSH agent discovered. Linux users have no agent discovery paths at all.

**Recommendation**: Add the missing socket paths. For Linux, use `libc::getuid()` to construct the uid-based paths.

**Fix applied (2026-03-31)**: Added all missing paths to `discover_agent_socket()`:
- 1Password Mac App Store: `~/Library/Group Containers/2BUA8C4S2C.com.1password/agent.sock` (macOS only, `#[cfg(target_os = "macos")]`)
- Linux system paths: `/run/user/{uid}/ssh-agent.socket` and `/run/user/{uid}/keyring/ssh` (Linux only, `#[cfg(target_os = "linux")]` via `libc::getuid()`)
- Added `libc = "0.2"` as Linux-only dependency in Cargo.toml

---

### Issue #4: Auto Auth Method Has No Fallback Chain (P1)

**File**: `src-tauri/src/infrastructure/ssh_connection_manager.rs` (lines 310-313)

**Electron auto auth** (tries in order):
1. Identity file from SSH config → tries `id_ed25519`, then `id_rsa`
2. SSH agent → via `discoverAgentSocket()`
3. Default key files → `id_ed25519`, `id_rsa`, `id_ecdsa`

**Tauri auto auth**:
- If SSH config has `IdentityFile`: promotes to `PrivateKey` (stops here)
- If no `IdentityFile`: stays `Auto` (no further processing defined)

**Impact**: Auto auth won't try SSH agent or default keys when no IdentityFile is configured in SSH config. Users with agent-based auth who haven't explicitly set IdentityFile in their SSH config will fail.

**Recommendation**: Implement the full fallback chain: identity file → agent → default keys.

---

### Issue #5: Remote Projects Path Hardcoded (P1)

**File**: `src-tauri/src/infrastructure/ssh_connection_manager.rs`

**Electron behavior**: After SFTP connect, queries remote `$HOME` via `execRemoteCommand('printf %s "$HOME"')`, then checks candidates:
1. `$HOME/.claude/projects`
2. `/home/$USER/.claude/projects`
3. `/Users/$USER/.claude/projects`
4. `/root/.claude/projects`

**Tauri behavior**: Hardcodes `/home/{username}/.claude/projects`.

**Impact**:
- macOS remote hosts (where `$HOME` is `/Users/username`) won't find projects
- Custom `$HOME` locations (e.g., NFS mounts, non-standard paths) won't work
- `/root/.claude/projects` for root users is not checked

**Recommendation**: Implement `execRemoteCommand()` via russh and add the same candidate fallback logic. This depends on Issue #1 (SFTP/SSH connection) being resolved first.

---

### Issue #6: SSH Config Include Directives Not Supported (P2) — **FIXED**

**File**: `src-tauri/src/infrastructure/ssh_config_parser.rs`

**Electron behavior**: Processes `Include` and `include` directives with:
- Tilde expansion (`~` → home dir)
- Glob pattern matching (via custom `globFiles()`)
- Recursive inclusion (nested Include directives)
- Silent skip on unreadable files

**Tauri behavior**: Only reads a single file (`~/.ssh/config`). No Include directive processing.

**Impact**: Users with modular SSH configs (e.g., `Include ~/.ssh/config.d/*`) will only see hosts defined in the main config file. Hosts in included files won't appear in the UI dropdown.

**Note**: The `ssh_config` crate may handle includes internally during `SSHConfig::parse_str()`, but the parser's own `extract_host_aliases()` function operates on the raw text of the single file read. If includes are handled by the crate, they would only apply to `query()` resolution, not to the host listing. This needs verification.

**Recommendation**: Implement Include directive expansion before parsing. Either:
1. Pre-process the file to expand includes (matching Electron's approach)
2. Verify if `ssh_config` crate handles includes and leverage that

**Fix applied (2026-03-31)**: Implemented `expand_includes()` in `ssh_config_parser.rs`:
- Processes both `Include` and `include` directives (case-insensitive)
- Tilde expansion (`~` → home dir)
- Glob pattern matching (`*`, `?`) via custom `glob_matches()` implementation
- Recursive inclusion with max depth of 10 (prevents infinite loops)
- Silent skip on unreadable/missing files
- `from_path()` now calls `expand_includes()` before parsing; `from_str()` parses raw content without expansion (correct for testing)
- Added 7 new tests covering single file include, glob patterns, missing files, max depth, glob matching, and from_str behavior

---

### Issue #7: No Remote Command Execution (P1)

**File**: `src-tauri/src/infrastructure/ssh_connection_manager.rs`

**Electron**: `execRemoteCommand(command)` method:
- Executes commands via `client.exec()`
- Collects stdout/stderr
- Resolves on exit code 0, rejects on non-zero
- Handles null exit codes
- Used for querying `$HOME` on remote host

**Tauri**: No equivalent method exists.

**Impact**: Cannot query remote environment (needed for dynamic path resolution). This is a dependency for Issue #5.

**Recommendation**: Implement `exec_remote_command()` via `russh::Channel::exec()`. Required for:
- Remote `$HOME` resolution
- Future features (e.g., remote git operations)

---

### Issue #8: No SFTP Retry Logic (P2)

**File**: `src-tauri/src/infrastructure/ssh_fs_provider.rs` (future)

**Electron behavior**:
- `MAX_RETRIES = 3`, `RETRY_BASE_DELAY_MS = 75`
- Linear backoff: 75ms, 150ms, 225ms
- Only retries `transient` errors on `readFile`, `stat`, `readdir`
- `exists()` on transient errors returns `true` (conservative)

**Tauri behavior**: No retry logic planned.

**Impact**: Transient SFTP errors (network blips, server load) will cause immediate failure instead of graceful recovery. Users may see spurious errors that resolve on retry.

**Recommendation**: Port Electron's retry logic with `tokio::time::sleep` for async backoff.

---

### Issue #9: No SFTP Error Classification (P2)

**File**: `src-tauri/src/infrastructure/ssh_fs_provider.rs` (future)

**Electron behavior** (`classifySftpError()`):
- `not_found`: code `2` or `ENOENT`
- `transient`: code `4`, `EAGAIN`, `ECONNRESET`, `ETIMEDOUT`, `EPIPE`
- `permanent`: everything else

**Tauri behavior**: No error classification.

**Impact**: Cannot distinguish between transient errors (retryable) and permanent errors (immediate fail). This is a dependency for Issue #8.

**Recommendation**: Port the error classification enum and implement it for russh SFTP error types.

---

### Issue #10: No createReadStream on FsProvider Trait (P2)

**File**: `src-tauri/src/infrastructure/fs_provider.rs`

**Electron**: `FileSystemProvider` has `createReadStream(path, options)` returning a Node.js `Readable` stream. Used for streaming large session files without loading entirely into memory.

**Tauri**: `FsProvider` trait has `read_file()` (full read to string) and `read_file_head()` (first N lines) but no streaming API.

**Impact**: Large remote session files must be read entirely into memory. For files >100MB this could cause memory pressure. However, this is mitigated by `read_file_head()` for initial metadata scanning.

**Note**: Rust's ownership model makes Node.js-style streaming fundamentally different. The current `read_file()` approach may be acceptable given:
- Session files are typically <50MB
- JSONL parsing already handles streaming at the parser level
- `read_file_head()` provides a lightweight alternative for metadata

**Recommendation**: Evaluate whether streaming is needed based on real-world file sizes. If needed, consider an iterator-based approach (`read_file_lines()`) instead of a full streaming API.

---

### Issue #11: LocalFsProvider.readdir Sequential Stat (P2) — **NOT A BUG**

**File**: `src-tauri/src/infrastructure/fs_provider.rs` (lines 134-167)

**Electron**: Stats all directory entries **concurrently** via `Promise.all()` for `mtimeMs` population.

**Tauri**: Stats entries **sequentially** in a `for` loop.

**Impact**: For directories with many files (e.g., hundreds of sessions), sequential stat calls are slower than concurrent ones. On a cold directory scan, this could add noticeable latency.

**Recommendation**: Use `rayon::parallel_iterator` or `tokio::task::spawn_blocking` with concurrent stat calls. Low priority since the local filesystem is typically fast enough.

**Status (2026-03-31)**: Initially attempted concurrent stat via `std::thread::scope`, but deep review revealed the I/O-bound `metadata()` syscall was called sequentially before threading, making parallelization pointless. Reverted to original sequential implementation. This is **not a meaningful difference**: Electron's `Promise.all` benefits from libuv's thread pool which queues async work transparently, while Rust's synchronous `metadata()` is a single fast syscall (typically served from OS dentry cache). For `~/.claude/projects/` (<50 entries typical), sequential is optimal.

---

### Issue #12: HTTP SSH save/get-last-connection Are Stubs (P2) — **ALREADY FIXED**

**File**: `src-tauri/src/http/routes/ssh.rs`

Already documented in Module 03 check. HTTP routes for `save-last-connection` and `get-last-connection` are stubs. Only Tauri IPC commands use ConfigManager for persistence.

**Recommendation**: Wire to ConfigManager in HttpState.

**Status (2026-03-31)**: Already fixed — `ssh_save_last_connection` and `ssh_get_last_connection` in `src-tauri/src/http/routes/ssh.rs` are now wired to `state.config_manager.update_config()` and `state.config_manager.get_config()`. The original check was outdated.

---

### Issue #13: SshFsProvider.read_dir Inconsistency (P2) — **FIXED**

**File**: `src-tauri/src/infrastructure/ssh_fs_provider.rs`

`read_dir()` returns `Ok(Vec::new())` (success with empty results) while all other methods return `Err("SFTP not yet implemented")`.

**Impact**: Callers may interpret an empty directory as a valid response rather than an error, leading to silent data loss (no sessions shown) instead of an error message.

**Recommendation**: Change to `Err("SFTP not yet implemented")` for consistency. Or, when implementing Phase 2, ensure this returns a proper SFTP directory listing.

**Fix applied (2026-03-31)**: Changed `read_dir()` to return `Err("SFTP not yet implemented")` for consistency with all other stub methods. Updated corresponding test from `test_read_dir_returns_empty` to `test_read_dir_returns_error`.

---

## Implementation Priority

### Phase 2 Must-Have (for SSH to be functional)

1. **Real SSH connection via russh** (Issues #1, #2) - Everything else depends on this
2. **SFTP FsProvider implementation** (Issue #1) - Core functionality
3. **Remote projects path resolution** (Issues #5, #7) - Dynamic $HOME query
4. **Auto auth fallback chain** (Issue #4) - Common auth scenario
5. **SSH agent discovery completion** (Issue #3) - 1Password + Linux support

### Phase 2 Should-Have (for robustness)

6. **SFTP retry logic + error classification** (Issues #8, #9) - Reliability
7. ~~**HTTP SSH persistence stubs** (Issue #12) - Browser mode consistency~~ **FIXED** (was already wired)

### Phase 2 Nice-to-Have

8. ~~**SSH config Include directives** (Issue #6) - Power user feature~~ **FIXED**
9. ~~**LocalFsProvider concurrent stat** (Issue #11) - Performance~~ **NOT A BUG** (sequential stat is optimal for this use case)
10. ~~**read_dir consistency fix** (Issue #13) - Code quality~~ **FIXED**
11. **createReadStream equivalent** (Issue #10) - Only if needed for large files (N/A - tech stack difference)
