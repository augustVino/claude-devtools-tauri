//! SSH SFTP 文件系统提供者 — 基于 russh-sftp 的完整实现。
//!
//! 通过 `SftpSession` 执行远程文件操作，使用 `tokio::task::block_in_place` +
//! `Handle::block_on()` 将异步 SFTP 调用安全桥接到同步的 `FsProvider` trait 方法。
//!
//! `block_in_place` 告知 tokio 当前工作线程将阻塞，调度器会将其他任务移至其他线程，
//! 从而避免在 tokio async 上下文中调用 `block_on` 导致的 panic。

use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::SftpSession;
#[allow(unused_imports)]
use russh_sftp::protocol::{Status as SftpStatus, StatusCode};
use std::path::Path;
use std::sync::Arc;

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

// ── SshFsProvider ──────────────────────────────────────────────────

/// SSH SFTP 文件系统提供者。
///
/// 包装 `SftpSession`，通过 `tokio::runtime::Handle` 将异步操作桥接到同步接口。
#[derive(Clone)]
pub struct SshFsProvider {
    sftp: Arc<SftpSession>,
    handle: tokio::runtime::Handle,
}

impl std::fmt::Debug for SshFsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshFsProvider")
            .field("sftp", &"Arc<SftpSession>")
            .field("handle", &self.handle)
            .finish()
    }
}

impl SshFsProvider {
    /// Create a new SSH FS provider wrapping an existing SFTP session.
    pub fn new(sftp: SftpSession, handle: tokio::runtime::Handle) -> Self {
        Self {
            sftp: Arc::new(sftp),
            handle,
        }
    }

    /// Execute an async SFTP operation, safe to call from any context.
    ///
    /// When inside a tokio runtime, uses `block_in_place` to avoid panicking
    /// on the same-runtime `block_on` call. When outside tokio (e.g. sync tests),
    /// falls back to direct `block_on`.
    fn blocking_sftp<F, T>(&self, f: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                // Inside a tokio runtime — use block_in_place to tell the
                // scheduler this thread will block, so it can move other
                // tasks to another worker thread.
                //
                // Safety: `handle` from try_current() is always the same
                // runtime as `self.handle`, because SshFsProvider is
                // constructed via new(sftp, Handle::current()) on the same
                // runtime. The cross-runtime block_on scenario cannot occur.
                tokio::task::block_in_place(|| handle.block_on(f))
            }
            Err(_) => {
                // Not inside a tokio runtime (e.g. plain unit test) —
                // safe to use handle.block_on directly.
                self.handle.block_on(f)
            }
        }
    }

    /// Async version of `exists` — safe to call from async contexts.
    ///
    /// Avoids the `block_on`-from-async panic by calling SFTP directly.
    pub async fn exists_async(&self, path: &str) -> Result<bool, SftpError> {
        self.sftp.try_exists(path).await
    }

    /// 异步关闭 SFTP 会话。在 async 上下文中调用。
    ///
    /// 不使用 `block_on()`，因为调用方（disconnect、健康监控）
    /// 都是 async 函数，`block_on()` 会导致 panic。
    /// 错误仅记录 warn 日志，不传播（会话可能已关闭）。
    pub async fn dispose_async(&self) {
        if let Err(e) = self.sftp.close().await {
            log::warn!("SFTP close error (may be already closed): {}", e);
        }
    }

    /// Get a clone of the inner SFTP session Arc.
    /// Used by health monitor for SFTP probes.
    pub fn sftp_arc(&self) -> Arc<SftpSession> {
        self.sftp.clone()
    }
}

impl FsProvider for SshFsProvider {
    fn provider_type(&self) -> &'static str {
        "ssh"
    }

    fn exists(&self, path: &Path) -> Result<bool, String> {
        let path_str = path.to_string_lossy().to_string();
        match self.blocking_sftp(self.sftp.try_exists(&path_str)) {
            Ok(exists) => Ok(exists),
            Err(err) => {
                let kind = classify_sftp_error(&err);
                match kind {
                    // 明确不存在 → false
                    SftpErrorKind::NotFound => Ok(false),
                    // 暂时性错误 → 保守返回 true（避免误判路径不存在）
                    SftpErrorKind::Transient => {
                        log::warn!(
                            "SFTP exists transient error for {}: {:?}",
                            path.display(),
                            err
                        );
                        Ok(true)
                    }
                    // 永久性错误 → false
                    SftpErrorKind::Permanent => {
                        log::warn!(
                            "SFTP exists permanent error for {}: {:?}",
                            path.display(),
                            err
                        );
                        Ok(false)
                    }
                }
            }
        }
    }

    fn read_file(&self, path: &Path) -> Result<String, String> {
        let path_str = path.to_string_lossy().to_string();
        let path_for_log = path.to_path_buf();
        let sftp = self.sftp.clone();
        crate::utils::retry::retry_transient(
            "SFTP read_file",
            path,
            || {
                let bytes = self.blocking_sftp(sftp.read(&path_str))?;
                String::from_utf8(bytes).map_err(|e| {
                    SftpError::IO(format!(
                        "UTF-8 decode error for {}: {}",
                        path_for_log.display(),
                        e
                    ))
                })
            },
            MAX_RETRIES,
            RETRY_BASE_DELAY_MS,
            &|err| classify_sftp_error(err) == SftpErrorKind::Transient,
        )
        .map_err(|e| format_sftp_error(path, &e))
    }

    fn read_file_head(&self, path: &Path, max_lines: usize) -> Result<String, String> {
        // 流式读前 64KB，避免读全文（对齐 Electron createReadStream）。
        // 64KB 通常覆盖 200+ 行 JSONL。
        //
        // 修正 codex 第二轮 C4：用 from_utf8_lossy 避免 CJK/emoji 字符在 64KB 边界
        // 被切断导致 from_utf8 失败 → 回退到全读 → 丢失截断保护。
        const HEAD_CHUNK_BYTES: u64 = 64 * 1024;

        // 拉取前 64KB 字节
        let bytes = match self.read_file_range(path, 0, Some(HEAD_CHUNK_BYTES)) {
            Ok(b) => b,
            Err(_) => {
                // SFTP 服务器不支持 seek 或 read with length → 回退到全读
                let full = self.read_file(path)?;
                full.into_bytes()
            }
        };

        // 关键：判断是否触发了截断（read_file_range 返回的字节数 == HEAD_CHUNK_BYTES）
        let was_truncated = (bytes.len() as u64) >= HEAD_CHUNK_BYTES;

        // 用 from_utf8_lossy 而非 from_utf8，避免 CJK 字符在 64KB 边界被切坏抛错。
        // lossy 在边界处用 U+FFFD 替换无效字节，不影响行解析（lines 按 \n 分割）。
        let mut content = String::from_utf8_lossy(&bytes).into_owned();

        // 截断保护：若读满 64KB 且末尾不是换行，丢弃最后一行（可能是不完整行）
        if was_truncated && !content.ends_with('\n') {
            if let Some(last_newline) = content.rfind('\n') {
                content.truncate(last_newline + 1);
            } else {
                // 整个 64KB 是一行（极少见，单行 JSONL > 64KB）→ 清空，让上层用其他方式读取
                log::warn!(
                    "read_file_head: 64KB chunk is a single line for {}, content truncated",
                    path.display()
                );
                content.clear();
            }
        }

        let lines: Vec<&str> = content.lines().take(max_lines).collect();
        Ok(lines.join("\n"))
    }

    fn read_file_range(
        &self,
        path: &Path,
        offset: u64,
        length: Option<u64>,
    ) -> Result<Vec<u8>, String> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let path_str = path.to_string_lossy().to_string();
        let path_for_log = path.to_path_buf();
        let sftp = self.sftp.clone();
        let offset_for_closure = offset;
        let length_for_closure = length;

        crate::utils::retry::retry_transient(
            "SFTP read_file_range",
            path,
            || {
                // 构造 future（async block 只捕获 sftp: Arc + 拷贝变量，不捕获 &self）
                let fut = async {
                    let mut file = sftp.open(&path_str).await.map_err(|e| {
                        SftpError::IO(format!("open {} failed: {}", path_for_log.display(), e))
                    })?;
                    if offset_for_closure > 0 {
                        file.seek(tokio::io::SeekFrom::Start(offset_for_closure))
                            .await
                            .map_err(|e| {
                                SftpError::IO(format!(
                                    "seek {} failed: {}",
                                    path_for_log.display(),
                                    e
                                ))
                            })?;
                    }
                    let mut buf = Vec::new();
                    match length_for_closure {
                        Some(n) => {
                            (&mut file).take(n).read_to_end(&mut buf).await.map_err(|e| {
                                SftpError::IO(format!("read range failed: {}", e))
                            })?;
                        }
                        None => {
                            file.read_to_end(&mut buf).await.map_err(|e| {
                                SftpError::IO(format!("read to end failed: {}", e))
                            })?;
                        }
                    }
                    Ok::<Vec<u8>, SftpError>(buf)
                };
                // self.blocking_sftp 在 closure 内调用（&self 借用不跨 closure 调用），
                // 与现有 read_file 模式相同。
                let bytes = self.blocking_sftp(fut)?;
                Ok::<Vec<u8>, SftpError>(bytes)
            },
            MAX_RETRIES,
            RETRY_BASE_DELAY_MS,
            &|err| classify_sftp_error(err) == SftpErrorKind::Transient,
        )
        .map_err(|e| format_sftp_error(path, &e))
    }

    fn stat(&self, path: &Path) -> Result<FsStatResult, String> {
        let path_str = path.to_string_lossy().to_string();
        let sftp = self.sftp.clone();
        crate::utils::retry::retry_transient(
            "SFTP stat",
            path,
            || {
                self.blocking_sftp(sftp.metadata(&path_str))
                    .map(|metadata| FsStatResult {
                        size: metadata.len(),
                        mtime_ms: crate::utils::time::time_to_ms(metadata.modified().ok()),
                        birthtime_ms: 0,
                        is_file: metadata.is_regular(),
                        is_directory: metadata.is_dir(),
                    })
            },
            MAX_RETRIES,
            RETRY_BASE_DELAY_MS,
            &|err| classify_sftp_error(err) == SftpErrorKind::Transient,
        )
        .map_err(|e| format_sftp_error(path, &e))
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<FsDirent>, String> {
        let path_str = path.to_string_lossy().to_string();
        let sftp = self.sftp.clone();
        crate::utils::retry::retry_transient(
            "SFTP read_dir",
            path,
            || {
                self.blocking_sftp(sftp.read_dir(&path_str))
                    .map(|read_dir| {
                        read_dir
                            .map(|entry| {
                                let meta = entry.metadata();
                                FsDirent {
                                    name: entry.file_name(),
                                    is_file: entry.file_type().is_file(),
                                    is_directory: entry.file_type().is_dir(),
                                    size: Some(meta.len()),
                                    mtime_ms: Some(crate::utils::time::time_to_ms(
                                        meta.modified().ok(),
                                    )),
                                    birthtime_ms: None,
                                }
                            })
                            .collect()
                    })
            },
            MAX_RETRIES,
            RETRY_BASE_DELAY_MS,
            &|err| classify_sftp_error(err) == SftpErrorKind::Transient,
        )
        .map_err(|e| format_sftp_error(path, &e))
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
        let ms = crate::utils::time::time_to_ms(Some(now));
        assert!(ms > 1_700_000_000_000); // sanity: after 2023

        assert_eq!(crate::utils::time::time_to_ms(None), 0);
    }
}
