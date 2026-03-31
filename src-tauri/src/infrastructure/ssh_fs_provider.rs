//! SSH SFTP 文件系统提供者 — 基于 russh-sftp 的完整实现。
//!
//! 通过 `SftpSession` 执行远程文件操作，使用 `tokio::runtime::Handle::block_on()`
//! 将异步 SFTP 调用桥接到同步的 `FsProvider` trait 方法。

use std::path::Path;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use russh_sftp::client::SftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::{Status as SftpStatus, StatusCode};

use crate::infrastructure::fs_provider::{FsDirent, FsProvider, FsStatResult};

// ── 常量 ───────────────────────────────────────────────────────────

/// 最大重试次数（不含首次尝试）。
const MAX_RETRIES: u32 = 3;
/// 首次重试延迟（毫秒），线性递增。
const RETRY_BASE_DELAY_MS: u64 = 75;

// ── 错误分类 ───────────────────────────────────────────────────────

/// SFTP 错误的可恢复性分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SftpErrorKind {
    /// 文件或目录不存在 — 无需重试。
    NotFound,
    /// 暂时性错误 — 可重试（如 SSH_FX_FAILURE、IO 超时等）。
    Transient,
    /// 永久性错误 — 不应重试。
    Permanent,
}

/// 根据 SFTP 错误类型分类可恢复性。
fn classify_sftp_error(err: &SftpError) -> SftpErrorKind {
    match err {
        SftpError::Status(status) => match status.status_code {
            StatusCode::NoSuchFile => SftpErrorKind::NotFound,
            StatusCode::Failure => SftpErrorKind::Transient,
            _ => SftpErrorKind::Permanent,
        },
        SftpError::IO(_) | SftpError::Timeout | SftpError::UnexpectedBehavior(_) => {
            SftpErrorKind::Transient
        }
        _ => SftpErrorKind::Permanent,
    }
}

/// 将 `SftpError` 转换为用户可读的错误字符串。
fn format_sftp_error(path: &Path, err: &SftpError) -> String {
    match err {
        SftpError::Status(status) => format!(
            "SFTP error for {}: {} (status {:?})",
            path.display(),
            status.error_message,
            status.status_code
        ),
        SftpError::IO(msg) => format!("SFTP IO error for {}: {}", path.display(), msg),
        SftpError::Timeout => format!("SFTP timeout for {}", path.display()),
        SftpError::Limited(msg) => format!("SFTP limited error for {}: {}", path.display(), msg),
        SftpError::UnexpectedPacket => format!("SFTP unexpected packet for {}", path.display()),
        SftpError::UnexpectedBehavior(msg) => {
            format!("SFTP unexpected behavior for {}: {}", path.display(), msg)
        }
    }
}

// ── 辅助：时间转换 ─────────────────────────────────────────────────

/// 将 `std::time::SystemTime` 转换为毫秒时间戳。
fn time_to_ms(time: std::io::Result<std::time::SystemTime>) -> u64 {
    match time {
        Ok(t) => t
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        Err(_) => 0,
    }
}

// ── SshFsProvider ──────────────────────────────────────────────────

/// SSH SFTP 文件系统提供者。
///
/// 包装 `SftpSession`，通过 `tokio::runtime::Handle` 将异步操作桥接到同步接口。
/// 内部使用 `Option` 以支持未连接的占位状态（过渡期兼容）。
#[derive(Clone)]
pub struct SshFsProvider {
    sftp: Option<Arc<SftpSession>>,
    handle: tokio::runtime::Handle,
}

impl std::fmt::Debug for SshFsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshFsProvider")
            .field(
                "sftp",
                &if self.sftp.is_some() {
                    "Some(Arc<SftpSession>)"
                } else {
                    "None"
                },
            )
            .field("handle", &self.handle)
            .finish()
    }
}

impl SshFsProvider {
    /// Create a new SSH FS provider wrapping an existing SFTP session.
    pub fn new(sftp: SftpSession, handle: tokio::runtime::Handle) -> Self {
        Self {
            sftp: Some(Arc::new(sftp)),
            handle,
        }
    }

    /// Phase 1 compatibility constructor — used by callers not yet updated to pass a real SFTP session.
    ///
    /// Creates a provider with no SFTP session. All `FsProvider` methods will return errors.
    /// **Deprecated**: Will be removed when all call sites are updated (Task 6).
    #[deprecated(note = "Use SshFsProvider::new(sftp, handle) instead")]
    pub fn new_placeholder(_host: String, _port: u16, _username: String) -> Self {
        let handle = tokio::runtime::Handle::current();
        Self { sftp: None, handle }
    }

    /// Get a reference to the inner SFTP session, or return an error if not connected.
    fn session(&self, path: &Path) -> Result<&Arc<SftpSession>, String> {
        self.sftp
            .as_ref()
            .ok_or_else(|| format!("No SFTP session for {}", path.display()))
    }

    /// Block on an async SFTP operation using the stored tokio runtime handle.
    fn block_on<F, T>(&self, f: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        self.handle.block_on(f)
    }
}

impl FsProvider for SshFsProvider {
    fn provider_type(&self) -> &'static str {
        "ssh"
    }

    fn exists(&self, path: &Path) -> Result<bool, String> {
        let sftp = self.session(path)?;
        let path_str = path.to_string_lossy().to_string();
        match self.block_on(sftp.try_exists(&path_str)) {
            Ok(exists) => Ok(exists),
            Err(err) => {
                let kind = classify_sftp_error(&err);
                match kind {
                    // 明确不存在 → false
                    SftpErrorKind::NotFound => Ok(false),
                    // 暂时性错误 → 保守返回 true（避免误判路径不存在）
                    SftpErrorKind::Transient => {
                        log::warn!("SFTP exists transient error for {}: {:?}", path.display(), err);
                        Ok(true)
                    }
                    // 永久性错误 → false
                    SftpErrorKind::Permanent => {
                        log::warn!("SFTP exists permanent error for {}: {:?}", path.display(), err);
                        Ok(false)
                    }
                }
            }
        }
    }

    fn read_file(&self, path: &Path) -> Result<String, String> {
        let sftp = self.session(path)?;
        let path_str = path.to_string_lossy().to_string();
        let mut last_err = None;

        for attempt in 0..=MAX_RETRIES {
            match self.block_on(sftp.read(&path_str)) {
                Ok(bytes) => {
                    return String::from_utf8(bytes).map_err(|e| {
                        format!("Failed to decode UTF-8 for {}: {}", path.display(), e)
                    });
                }
                Err(err) => {
                    let kind = classify_sftp_error(&err);
                    if kind == SftpErrorKind::NotFound {
                        return Err(format_sftp_error(path, &err));
                    }
                    if attempt < MAX_RETRIES && kind == SftpErrorKind::Transient {
                        let delay = RETRY_BASE_DELAY_MS * (attempt as u64 + 1);
                        log::warn!(
                            "SFTP read_file transient error (attempt {}/{}) for {}: {:?}, retrying in {}ms",
                            attempt + 1,
                            MAX_RETRIES + 1,
                            path.display(),
                            err,
                            delay
                        );
                        std::thread::sleep(std::time::Duration::from_millis(delay));
                        last_err = Some(err);
                        continue;
                    }
                    return Err(format_sftp_error(path, &err));
                }
            }
        }

        Err(format_sftp_error(
            path,
            last_err.as_ref().unwrap_or(&SftpError::UnexpectedBehavior(
                "unexpected loop exit".into(),
            )),
        ))
    }

    fn read_file_head(&self, path: &Path, max_lines: usize) -> Result<String, String> {
        let content = self.read_file(path)?;
        let lines: Vec<&str> = content.lines().take(max_lines).collect();
        Ok(lines.join("\n"))
    }

    fn stat(&self, path: &Path) -> Result<FsStatResult, String> {
        let sftp = self.session(path)?;
        let path_str = path.to_string_lossy().to_string();
        let mut last_err = None;

        for attempt in 0..=MAX_RETRIES {
            match self.block_on(sftp.metadata(&path_str)) {
                Ok(metadata) => {
                    return Ok(FsStatResult {
                        size: metadata.len(),
                        mtime_ms: time_to_ms(metadata.modified()),
                        birthtime_ms: 0, // SFTP FileAttributes 不提供 birthtime
                        is_file: metadata.is_regular(),
                        is_directory: metadata.is_dir(),
                    });
                }
                Err(err) => {
                    let kind = classify_sftp_error(&err);
                    if kind == SftpErrorKind::NotFound {
                        return Err(format_sftp_error(path, &err));
                    }
                    if attempt < MAX_RETRIES && kind == SftpErrorKind::Transient {
                        let delay = RETRY_BASE_DELAY_MS * (attempt as u64 + 1);
                        log::warn!(
                            "SFTP stat transient error (attempt {}/{}) for {}: {:?}, retrying in {}ms",
                            attempt + 1,
                            MAX_RETRIES + 1,
                            path.display(),
                            err,
                            delay
                        );
                        std::thread::sleep(std::time::Duration::from_millis(delay));
                        last_err = Some(err);
                        continue;
                    }
                    return Err(format_sftp_error(path, &err));
                }
            }
        }

        Err(format_sftp_error(
            path,
            last_err.as_ref().unwrap_or(&SftpError::UnexpectedBehavior(
                "unexpected loop exit".into(),
            )),
        ))
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<FsDirent>, String> {
        let sftp = self.session(path)?;
        let path_str = path.to_string_lossy().to_string();
        let mut last_err = None;

        for attempt in 0..=MAX_RETRIES {
            match self.block_on(sftp.read_dir(&path_str)) {
                Ok(read_dir) => {
                    let entries: Vec<FsDirent> = read_dir
                        .map(|entry| {
                            let meta = entry.metadata();
                            FsDirent {
                                name: entry.file_name(),
                                is_file: entry.file_type().is_file(),
                                is_directory: entry.file_type().is_dir(),
                                size: Some(meta.len()),
                                mtime_ms: Some(time_to_ms(meta.modified())),
                                birthtime_ms: None, // SFTP 不提供 birthtime
                            }
                        })
                        .collect();
                    return Ok(entries);
                }
                Err(err) => {
                    let kind = classify_sftp_error(&err);
                    if kind == SftpErrorKind::NotFound {
                        return Err(format_sftp_error(path, &err));
                    }
                    if attempt < MAX_RETRIES && kind == SftpErrorKind::Transient {
                        let delay = RETRY_BASE_DELAY_MS * (attempt as u64 + 1);
                        log::warn!(
                            "SFTP read_dir transient error (attempt {}/{}) for {}: {:?}, retrying in {}ms",
                            attempt + 1,
                            MAX_RETRIES + 1,
                            path.display(),
                            err,
                            delay
                        );
                        std::thread::sleep(std::time::Duration::from_millis(delay));
                        last_err = Some(err);
                        continue;
                    }
                    return Err(format_sftp_error(path, &err));
                }
            }
        }

        Err(format_sftp_error(
            path,
            last_err.as_ref().unwrap_or(&SftpError::UnexpectedBehavior(
                "unexpected loop exit".into(),
            )),
        ))
    }
}

// ── 测试 ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── 错误分类 ───────────────────────────────────────────────

    #[test]
    fn test_classify_not_found() {
        let err = SftpError::Status(SftpStatus {
            id: 0,
            status_code: StatusCode::NoSuchFile,
            error_message: "no such file".into(),
            language_tag: "".into(),
        });
        assert_eq!(classify_sftp_error(&err), SftpErrorKind::NotFound);
    }

    #[test]
    fn test_classify_failure_is_transient() {
        let err = SftpError::Status(SftpStatus {
            id: 0,
            status_code: StatusCode::Failure,
            error_message: "generic failure".into(),
            language_tag: "".into(),
        });
        assert_eq!(classify_sftp_error(&err), SftpErrorKind::Transient);
    }

    #[test]
    fn test_classify_permission_denied_is_permanent() {
        let err = SftpError::Status(SftpStatus {
            id: 0,
            status_code: StatusCode::PermissionDenied,
            error_message: "permission denied".into(),
            language_tag: "".into(),
        });
        assert_eq!(classify_sftp_error(&err), SftpErrorKind::Permanent);
    }

    #[test]
    fn test_classify_io_error_is_transient() {
        let err = SftpError::IO("connection reset".into());
        assert_eq!(classify_sftp_error(&err), SftpErrorKind::Transient);
    }

    #[test]
    fn test_classify_timeout_is_transient() {
        let err = SftpError::Timeout;
        assert_eq!(classify_sftp_error(&err), SftpErrorKind::Transient);
    }

    #[test]
    fn test_classify_unexpected_behavior_is_transient() {
        let err = SftpError::UnexpectedBehavior("bad state".into());
        assert_eq!(classify_sftp_error(&err), SftpErrorKind::Transient);
    }

    #[test]
    fn test_classify_unexpected_packet_is_permanent() {
        let err = SftpError::UnexpectedPacket;
        assert_eq!(classify_sftp_error(&err), SftpErrorKind::Permanent);
    }

    // ── Debug 格式 ─────────────────────────────────────────────

    #[test]
    fn test_debug_format_compiles() {
        // Verify the Debug trait is implemented for SshFsProvider.
        // We cannot construct a real instance without a live SFTP session,
        // but we verify the impl compiles and produces the expected struct name
        // by checking that the type satisfies the Debug trait bound.
        fn assert_debug<T: std::fmt::Debug>() {}
        assert_debug::<SshFsProvider>();
    }

    #[test]
    fn test_debug_format_placeholder() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        #[allow(deprecated)]
        let p = SshFsProvider::new_placeholder("host".into(), 22, "user".into());
        let debug_str = format!("{:?}", p);
        assert!(debug_str.contains("SshFsProvider"));
        assert!(debug_str.contains("None"));
    }

    // ── 错误分类覆盖率 ─────────────────────────────────────────

    #[test]
    fn test_error_classification_coverage() {
        // Verify all StatusCode variants are handled:
        // NoSuchFile -> NotFound, Failure -> Transient, others -> Permanent
        let not_found = SftpError::Status(SftpStatus {
            id: 1,
            status_code: StatusCode::NoSuchFile,
            error_message: "".into(),
            language_tag: "".into(),
        });
        let failure = SftpError::Status(SftpStatus {
            id: 2,
            status_code: StatusCode::Failure,
            error_message: "".into(),
            language_tag: "".into(),
        });
        let perm_denied = SftpError::Status(SftpStatus {
            id: 3,
            status_code: StatusCode::PermissionDenied,
            error_message: "".into(),
            language_tag: "".into(),
        });

        assert_eq!(classify_sftp_error(&not_found), SftpErrorKind::NotFound);
        assert_eq!(classify_sftp_error(&failure), SftpErrorKind::Transient);
        assert_eq!(classify_sftp_error(&perm_denied), SftpErrorKind::Permanent);
    }

    // ── 错误格式化 ─────────────────────────────────────────────

    #[test]
    fn test_format_sftp_error_status() {
        let err = SftpError::Status(SftpStatus {
            id: 1,
            status_code: StatusCode::NoSuchFile,
            error_message: "file not found".into(),
            language_tag: "en".into(),
        });
        let msg = format_sftp_error(Path::new("/tmp/test"), &err);
        assert!(msg.contains("/tmp/test"));
        assert!(msg.contains("file not found"));
        assert!(msg.contains("NoSuchFile"));
    }

    #[test]
    fn test_format_sftp_error_timeout() {
        let err = SftpError::Timeout;
        let msg = format_sftp_error(Path::new("/tmp/test"), &err);
        assert!(msg.contains("timeout"));
        assert!(msg.contains("/tmp/test"));
    }

    // ── 时间转换 ───────────────────────────────────────────────

    #[test]
    fn test_time_to_ms() {
        let now = std::time::SystemTime::now();
        let ms = time_to_ms(Ok(now));
        assert!(ms > 1_700_000_000_000); // sanity: after 2023

        let err_time = Err(std::io::Error::new(std::io::ErrorKind::Other, "no time"));
        assert_eq!(time_to_ms(err_time), 0);
    }

    // ── 占位符提供者（无 SFTP 会话） ────────────────────────────

    #[test]
    fn test_placeholder_provider_type() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        #[allow(deprecated)]
        let p = SshFsProvider::new_placeholder("h".into(), 22, "u".into());
        assert_eq!(p.provider_type(), "ssh");
    }

    #[test]
    fn test_placeholder_returns_error_on_read_file() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        #[allow(deprecated)]
        let p = SshFsProvider::new_placeholder("h".into(), 22, "u".into());
        let result = p.read_file(Path::new("/some/file"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No SFTP session"));
    }

    #[test]
    fn test_placeholder_returns_error_on_stat() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        #[allow(deprecated)]
        let p = SshFsProvider::new_placeholder("h".into(), 22, "u".into());
        let result = p.stat(Path::new("/some/file"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No SFTP session"));
    }

    #[test]
    fn test_placeholder_returns_error_on_read_dir() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        #[allow(deprecated)]
        let p = SshFsProvider::new_placeholder("h".into(), 22, "u".into());
        let result = p.read_dir(Path::new("/some/dir"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No SFTP session"));
    }
}
