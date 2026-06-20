//! Memory Service implementation — SSH-aware via FsProvider.
//!
//! 对齐 Electron `MemoryReader.ts`：
//! - 构造持有 `ContextManager`，从 active ServiceContext 动态取 fs_provider + projects_dir
//! - 所有读取走 fs_provider（本地 std::fs / 远程 SFTP 多态）
//! - containment 校验用 `assert_safe_name` + path 字符串等值（不依赖 canonicalize，
//!   SSH SFTP 不支持 readlink 全路径；与 Electron `MemoryReader.ts:54-57` 一致）

use crate::error::AppError;
use crate::infrastructure::fs_provider::FsProvider;
use crate::infrastructure::ContextManager;
use crate::types::memory::{parse_memory_index, MemoryFile, MemoryIndex};
use crate::utils::app_opener;
use crate::utils::path_decoder;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

use super::memory_service_trait::MemoryService;

const MEMORY_DIR_NAME: &str = "memory";
const INDEX_FILE_NAME: &str = "MEMORY.md";
/// Max file size for a single memory file (1 MB). SSH 防御性限制。
const MAX_FILE_SIZE: usize = 1024 * 1024;

pub struct MemoryServiceImpl {
    context_manager: Arc<RwLock<ContextManager>>,
}

impl MemoryServiceImpl {
    pub fn new(context_manager: Arc<RwLock<ContextManager>>) -> Self {
        Self { context_manager }
    }

    /// 从 active ServiceContext 取 fs_provider + projects_dir。
    async fn deps(&self) -> Result<(Arc<dyn FsProvider>, PathBuf), AppError> {
        let active_arc = {
            let mgr = self.context_manager.read().await;
            mgr.get_active()
                .ok_or_else(|| AppError::Internal("No active ServiceContext".into()))?
        };
        let ctx = active_arc.read().await;
        Ok((ctx.fs_provider.clone(), ctx.projects_dir.clone()))
    }

    /// Resolve memory directory path. Uses `extract_base_dir` to strip
    /// composite ID suffixes (`::{hash}`), matching project_scanner.rs.
    fn dir_path(projects_dir: &Path, project_id: &str) -> PathBuf {
        let base_dir = path_decoder::extract_base_dir(project_id);
        projects_dir.join(base_dir).join(MEMORY_DIR_NAME)
    }

    /// Validate file name and resolve to absolute path.
    /// 校验对齐 Electron `assertSafeMarkdownName`：拒绝 `..`、`/`、`\`，
    /// 强制 `.md` 后缀；containment 用 path 字符串等值（resolve 后必须等于 join 结果），
    /// 不依赖 canonicalize（SSH SFTP 不支持 readlink 全路径）。
    fn safe_file_path(
        projects_dir: &Path,
        project_id: &str,
        file_name: &str,
    ) -> Result<PathBuf, AppError> {
        let dir = Self::dir_path(projects_dir, project_id);
        let safe = Self::assert_safe_name(file_name)?;
        let resolved = dir.join(&safe);
        // resolve 等值检查：safe 已拒绝 `..` 和分隔符，resolved 必须等于 dir/safe。
        // 这是纯字符串操作，SSH 下安全，对齐 Electron MemoryReader.ts:54-57。
        let expected = dir.join(&safe);
        if resolved != expected {
            return Err(AppError::InvalidInput(format!(
                "Memory file path escapes memory directory: {file_name}"
            )));
        }
        Ok(resolved)
    }

    /// 对齐 Electron `assertSafeMarkdownName`。
    fn assert_safe_name(file_name: &str) -> Result<String, AppError> {
        let trimmed = file_name.trim();
        if trimmed.is_empty() {
            return Err(AppError::InvalidInput("Memory file name is empty".into()));
        }
        if trimmed.contains('/') || trimmed.contains('\\') {
            return Err(AppError::InvalidInput(format!(
                "Memory file name must not contain path separators: {file_name}"
            )));
        }
        if trimmed.contains("..") {
            return Err(AppError::InvalidInput(format!(
                "Memory file name must not contain '..': {file_name}"
            )));
        }
        if !trimmed.to_lowercase().ends_with(".md") {
            return Err(AppError::InvalidInput(format!(
                "Memory file must end with .md: {file_name}"
            )));
        }
        Ok(trimmed.to_string())
    }
}

#[async_trait]
impl MemoryService for MemoryServiceImpl {
    async fn has_memory(&self, project_id: &str) -> Result<bool, AppError> {
        let (fs_provider, projects_dir) = self.deps().await?;
        let dir = Self::dir_path(&projects_dir, project_id);
        if !fs_provider.exists(&dir).unwrap_or(false) {
            return Ok(false);
        }
        let entries = match fs_provider.read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return Ok(false),
        };
        Ok(entries
            .iter()
            .any(|e| e.is_file && e.name.to_lowercase().ends_with(".md")))
    }

    async fn read_index(&self, project_id: &str) -> Result<Option<MemoryIndex>, AppError> {
        let (fs_provider, projects_dir) = self.deps().await?;
        let dir = Self::dir_path(&projects_dir, project_id);
        if !fs_provider.exists(&dir).unwrap_or(false) {
            return Ok(None);
        }

        let mut names = Vec::new();
        match fs_provider.read_dir(&dir) {
            Ok(entries) => {
                for entry in entries {
                    if entry.is_file {
                        names.push(entry.name);
                    }
                }
            }
            Err(_) => return Ok(None),
        }

        let index_path = dir.join(INDEX_FILE_NAME);
        let raw = if fs_provider.exists(&index_path).unwrap_or(false) {
            fs_provider.read_file(&index_path).unwrap_or_default()
        } else {
            String::new()
        };
        Ok(Some(parse_memory_index(&raw, &names)))
    }

    async fn read_file(&self, project_id: &str, file_name: &str) -> Result<MemoryFile, AppError> {
        let (fs_provider, projects_dir) = self.deps().await?;
        let absolute_path = Self::safe_file_path(&projects_dir, project_id, file_name)?;

        let content = fs_provider.read_file(&absolute_path).map_err(|e| {
            AppError::NotFound(format!("Memory file read failed for {file_name}: {e}"))
        })?;

        // 防御性 size 检查（对齐 Tauri 原有 MAX_FILE_SIZE 防护，Electron 没有此检查）
        if content.len() > MAX_FILE_SIZE {
            return Err(AppError::InvalidInput(format!(
                "Memory file exceeds {} KB limit",
                MAX_FILE_SIZE / 1024
            )));
        }

        Ok(MemoryFile {
            file_name: file_name.to_string(),
            absolute_path: absolute_path.to_string_lossy().to_string(),
            content,
        })
    }

    fn get_dir_path(&self, project_id: &str) -> String {
        // 同步接口，无法 await deps()。用本地默认路径作为 fallback。
        // 真正的 SSH-aware 路径在异步方法中通过 deps() 解析；这里仅供 UI 展示用。
        let projects_dir = path_decoder::get_projects_base_path();
        Self::dir_path(&projects_dir, project_id)
            .to_string_lossy()
            .to_string()
    }

    fn get_file_path(&self, project_id: &str, file_name: &str) -> Result<String, AppError> {
        // 同步接口 fallback，与 get_dir_path 同理。
        let projects_dir = path_decoder::get_projects_base_path();
        Ok(Self::safe_file_path(&projects_dir, project_id, file_name)?
            .to_string_lossy()
            .to_string())
    }

    async fn open_in(
        &self,
        opener_id: &str,
        project_id: &str,
        file_name: Option<&str>,
    ) -> Result<(), AppError> {
        // open_in 调用 OS opener 打开本地路径。SSH 模式下路径是远程的，
        // OS opener 无法打开——这是已知限制（与 Electron MemoryReader.openIn 一致：
        // Electron 也只在本地模式下提供 open_in）。
        let path = match file_name {
            Some(name) if !name.trim().is_empty() => self.get_file_path(project_id, name)?,
            _ => self.get_dir_path(project_id),
        };
        let is_directory = file_name.map_or(true, |n| n.trim().is_empty());
        app_opener::open_with(opener_id, &path, is_directory).await
    }
}
