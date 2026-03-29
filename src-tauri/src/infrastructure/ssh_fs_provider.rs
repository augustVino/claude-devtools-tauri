//! SSH SFTP 文件系统提供者 — Phase 1 占位实现。
//!
//! All methods return errors or empty results until the SFTP layer is implemented.

use std::path::Path;

use crate::infrastructure::fs_provider::{FsDirent, FsProvider, FsStatResult};

/// SSH SFTP 文件系统提供者。
///
/// Holds connection parameters for future SFTP session establishment.
/// Phase 1: all operations return placeholder errors.
#[derive(Debug)]
pub struct SshFsProvider {
    host: String,
    port: u16,
    username: String,
}

impl SshFsProvider {
    /// Create a new SSH FS provider with the given connection parameters.
    pub fn new(host: String, port: u16, username: String) -> Self {
        Self {
            host,
            port,
            username,
        }
    }

    /// Returns the configured host.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Returns the configured port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Returns the configured username.
    pub fn username(&self) -> &str {
        &self.username
    }
}

impl FsProvider for SshFsProvider {
    fn provider_type(&self) -> &'static str {
        "ssh"
    }

    fn exists(&self, _path: &Path) -> Result<bool, String> {
        Err("SFTP not yet implemented".into())
    }

    fn read_file(&self, _path: &Path) -> Result<String, String> {
        Err("SFTP not yet implemented".into())
    }

    fn read_file_head(&self, _path: &Path, _max_lines: usize) -> Result<String, String> {
        Err("SFTP not yet implemented".into())
    }

    fn stat(&self, _path: &Path) -> Result<FsStatResult, String> {
        Err("SFTP not yet implemented".into())
    }

    fn read_dir(&self, _path: &Path) -> Result<Vec<FsDirent>, String> {
        Ok(Vec::new())
    }
}

impl Drop for SshFsProvider {
    fn drop(&mut self) {
        // TODO: Clean up SFTP resources when implemented
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn provider() -> SshFsProvider {
        SshFsProvider::new("example.com".into(), 22, "user".into())
    }

    // ── construction ─────────────────────────────────────────────

    #[test]
    fn test_new() {
        let p = provider();
        assert_eq!(p.host(), "example.com");
        assert_eq!(p.port(), 22);
        assert_eq!(p.username(), "user");
    }

    #[test]
    fn test_provider_type() {
        let p = provider();
        assert_eq!(p.provider_type(), "ssh");
    }

    // ── placeholder errors ───────────────────────────────────────

    #[test]
    fn test_exists_returns_error() {
        let p = provider();
        let result = p.exists(PathBuf::from("/some/path").as_path());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "SFTP not yet implemented");
    }

    #[test]
    fn test_read_file_returns_error() {
        let p = provider();
        let result = p.read_file(PathBuf::from("/some/file").as_path());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "SFTP not yet implemented");
    }

    #[test]
    fn test_read_file_head_returns_error() {
        let p = provider();
        let result = p.read_file_head(PathBuf::from("/some/file").as_path(), 10);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "SFTP not yet implemented");
    }

    #[test]
    fn test_stat_returns_error() {
        let p = provider();
        let result = p.stat(PathBuf::from("/some/file").as_path());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "SFTP not yet implemented");
    }

    // ── placeholder empty ────────────────────────────────────────

    #[test]
    fn test_read_dir_returns_empty() {
        let p = provider();
        let result = p.read_dir(PathBuf::from("/some/dir").as_path());
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    // ── Debug trait ──────────────────────────────────────────────

    #[test]
    fn test_debug_format() {
        let p = provider();
        let debug = format!("{:?}", p);
        assert!(debug.contains("SshFsProvider"));
        assert!(debug.contains("example.com"));
    }
}
