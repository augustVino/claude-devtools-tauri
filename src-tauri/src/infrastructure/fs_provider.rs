//! 文件系统提供者抽象 — 支持本地文件系统和 SSH/SFTP。
//!
//! 该 trait 为同步接口，因为现有的发现模块（ProjectScanner、SessionSearcher 等）
//! 使用 `std::fs` 进行同步文件操作。SSH/SFTP 实现将通过内部线程池
//! 将异步 SSH 操作转换为同步调用。

use std::fs;
use std::io::BufRead;
use std::path::Path;

/// 文件元数据结果。
#[derive(Debug, Clone)]
pub struct FsStatResult {
    /// 文件大小（字节）
    pub size: u64,
    /// 修改时间（毫秒时间戳）
    pub mtime_ms: u64,
    /// 创建时间（毫秒时间戳）
    pub birthtime_ms: u64,
    /// 是否为文件
    pub is_file: bool,
    /// 是否为目录
    pub is_directory: bool,
}

/// 目录条目。
#[derive(Debug, Clone)]
pub struct FsDirent {
    /// 文件或目录名
    pub name: String,
    /// 是否为文件
    pub is_file: bool,
    /// 是否为目录
    pub is_directory: bool,
    /// 文件大小（字节），目录时为 None
    pub size: Option<u64>,
    /// 修改时间（毫秒时间戳）
    pub mtime_ms: Option<u64>,
    /// 创建时间（毫秒时间戳）
    pub birthtime_ms: Option<u64>,
}

/// 文件系统提供者 trait — 抽象本地和远程文件操作。
///
/// 所有方法均为同步，以匹配现有 `std::fs` 调用模式。
/// 实现 `Send + Sync` 以确保线程安全。
pub trait FsProvider: Send + Sync + std::fmt::Debug {
    /// 返回提供者类型标识（如 "local"、"ssh"）。
    fn provider_type(&self) -> &'static str;

    /// 检查路径是否存在。
    fn exists(&self, path: &Path) -> Result<bool, String>;

    /// 读取文件全部内容为字符串。
    fn read_file(&self, path: &Path) -> Result<String, String>;

    /// 读取文件前 N 行，适用于大文件的快速预览。
    fn read_file_head(&self, path: &Path, max_lines: usize) -> Result<String, String>;

    /// 获取文件元数据。
    fn stat(&self, path: &Path) -> Result<FsStatResult, String>;

    /// 列出目录内容。
    fn read_dir(&self, path: &Path) -> Result<Vec<FsDirent>, String>;

    /// 清理资源（如关闭 SFTP 会话）。默认为空操作。
    ///
    /// 对于需要异步清理的实现者（如 SshFsProvider），使用
    /// 自有的 `dispose_async()` 方法代替。
    fn dispose(&self) {}
}

/// 本地文件系统提供者 — 基于 `std::fs` 实现。
#[derive(Debug)]
pub struct LocalFsProvider;

impl LocalFsProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LocalFsProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl FsProvider for LocalFsProvider {
    fn provider_type(&self) -> &'static str {
        "local"
    }

    fn exists(&self, path: &Path) -> Result<bool, String> {
        Ok(path.exists())
    }

    fn read_file(&self, path: &Path) -> Result<String, String> {
        fs::read_to_string(path).map_err(|e| format!("Failed to read {}: {}", path.display(), e))
    }

    fn read_file_head(&self, path: &Path, max_lines: usize) -> Result<String, String> {
        let file = fs::File::open(path)
            .map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;
        let reader = std::io::BufReader::new(file);
        let mut lines: Vec<String> = Vec::with_capacity(max_lines);
        for line in reader.lines().take(max_lines) {
            match line {
                Ok(l) => lines.push(l),
                Err(e) => return Err(format!("Failed to read line in {}: {}", path.display(), e)),
            }
        }
        Ok(lines.join("\n"))
    }

    fn stat(&self, path: &Path) -> Result<FsStatResult, String> {
        let metadata = fs::metadata(path)
            .map_err(|e| format!("Failed to stat {}: {}", path.display(), e))?;
        Ok(FsStatResult {
            size: metadata.len(),
            mtime_ms: crate::utils::time::time_to_ms(metadata.modified().ok()),
            birthtime_ms: crate::utils::time::time_to_ms(metadata.created().ok()),
            is_file: metadata.is_file(),
            is_directory: metadata.is_dir(),
        })
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<FsDirent>, String> {
        let entries = fs::read_dir(path)
            .map_err(|e| format!("Failed to read dir {}: {}", path.display(), e))?;
        let mut result = Vec::new();
        for entry in entries {
            let entry = entry
                .map_err(|e| format!("Failed to read dir entry in {}: {}", path.display(), e))?;
            let name = entry
                .file_name()
                .to_string_lossy()
                .to_string();
            let metadata = entry.metadata().ok();
            let is_file = metadata.as_ref().is_some_and(|m| m.is_file());
            let is_directory = metadata.as_ref().is_some_and(|m| m.is_dir());
            let size = metadata.as_ref().map(|m| m.len());
            let mtime_ms = metadata
                .as_ref()
                .and_then(|m| m.modified().ok())
                .map(|t| crate::utils::time::time_to_ms(Some(t)));
            let birthtime_ms = metadata
                .as_ref()
                .and_then(|m| m.created().ok())
                .map(|t| crate::utils::time::time_to_ms(Some(t)));
            result.push(FsDirent {
                name,
                is_file,
                is_directory,
                size,
                mtime_ms,
                birthtime_ms,
            });
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn provider() -> LocalFsProvider {
        LocalFsProvider::new()
    }

    // ── exists ────────────────────────────────────────────────

    #[test]
    fn test_exists_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "hello").unwrap();

        let p = provider();
        assert!(p.exists(&file_path).unwrap());
    }

    #[test]
    fn test_exists_missing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nonexistent.txt");

        let p = provider();
        assert!(!p.exists(&missing).unwrap());
    }

    // ── read_file ─────────────────────────────────────────────

    #[test]
    fn test_read_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("greeting.txt");
        fs::write(&file_path, "hello world").unwrap();

        let p = provider();
        assert_eq!(p.read_file(&file_path).unwrap(), "hello world");
    }

    #[test]
    fn test_read_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.txt");

        let p = provider();
        assert!(p.read_file(&missing).is_err());
    }

    // ── read_file_head ────────────────────────────────────────

    #[test]
    fn test_read_file_head_fewer_lines_than_max() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("lines.txt");
        fs::write(&file_path, "line1\nline2\n").unwrap();

        let p = provider();
        let result = p.read_file_head(&file_path, 10).unwrap();
        assert_eq!(result, "line1\nline2");
    }

    #[test]
    fn test_read_file_head_truncates() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("many_lines.txt");
        let mut file = fs::File::create(&file_path).unwrap();
        for i in 0..10 {
            writeln!(file, "line{}", i).unwrap();
        }

        let p = provider();
        let result = p.read_file_head(&file_path, 3).unwrap();
        assert_eq!(result, "line0\nline1\nline2");
    }

    // ── stat ──────────────────────────────────────────────────

    #[test]
    fn test_stat_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("data.bin");
        fs::write(&file_path, b"12345").unwrap();

        let p = provider();
        let stat = p.stat(&file_path).unwrap();
        assert_eq!(stat.size, 5);
        assert!(stat.is_file);
        assert!(!stat.is_directory);
        assert!(stat.mtime_ms > 0);
    }

    #[test]
    fn test_stat_directory() {
        let dir = tempfile::tempdir().unwrap();

        let p = provider();
        let stat = p.stat(dir.path()).unwrap();
        assert!(stat.is_directory);
        assert!(!stat.is_file);
    }

    #[test]
    fn test_stat_missing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("ghost");

        let p = provider();
        assert!(p.stat(&missing).is_err());
    }

    // ── read_dir ──────────────────────────────────────────────

    #[test]
    fn test_read_dir() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "aaa").unwrap();
        fs::write(dir.path().join("b.txt"), "bbb").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();

        let p = provider();
        let entries = p.read_dir(dir.path()).unwrap();

        let mut names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["a.txt", "b.txt", "sub"]);

        let a = entries.iter().find(|e| e.name == "a.txt").unwrap();
        assert!(a.is_file);
        assert!(!a.is_directory);
        assert_eq!(a.size, Some(3));

        let sub = entries.iter().find(|e| e.name == "sub").unwrap();
        assert!(sub.is_directory);
        assert!(!sub.is_file);
    }

    #[test]
    fn test_read_dir_empty() {
        let dir = tempfile::tempdir().unwrap();

        let p = provider();
        let entries = p.read_dir(dir.path()).unwrap();
        assert!(entries.is_empty());
    }

    // ── provider_type ─────────────────────────────────────────

    #[test]
    fn test_provider_type() {
        let p = provider();
        assert_eq!(p.provider_type(), "local");
    }
}
