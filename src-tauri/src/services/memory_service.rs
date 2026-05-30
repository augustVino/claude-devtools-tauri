//! Memory Service implementation — reads from local filesystem.

use async_trait::async_trait;
use std::path::PathBuf;
use std::io::Read;
use crate::error::AppError;
use crate::types::memory::{MemoryFile, MemoryIndex, parse_memory_index};
use crate::utils::path_decoder;

use super::memory_service_trait::MemoryService;

const MEMORY_DIR_NAME: &str = "memory";
const INDEX_FILE_NAME: &str = "MEMORY.md";
/// Max file size for a single memory file (1 MB).
const MAX_FILE_SIZE: u64 = 1024 * 1024;

pub struct MemoryServiceImpl {
    projects_dir: PathBuf,
}

impl MemoryServiceImpl {
    pub fn new(projects_dir: PathBuf) -> Self {
        Self { projects_dir }
    }

    /// Resolve memory directory path. Uses `extract_base_dir` to strip
    /// composite ID suffixes (`::{hash}`), matching project_scanner.rs.
    fn dir_path_buf(&self, project_id: &str) -> PathBuf {
        let base_dir = path_decoder::extract_base_dir(project_id);
        self.projects_dir
            .join(path_decoder::decode_path(base_dir))
            .join(MEMORY_DIR_NAME)
    }

    /// Validate memory directory path via canonicalize + containment check.
    /// Used by `has_memory`, `read_index`, and `safe_file_path_buf` to
    /// ensure the resolved directory stays within the projects root.
    fn safe_dir_path_buf(&self, project_id: &str) -> Result<PathBuf, AppError> {
        let dir = self.dir_path_buf(project_id);
        let canonical_dir = std::fs::canonicalize(&dir)
            .map_err(|_| AppError::NotFound("Memory directory not found".into()))?;
        let canonical_projects = std::fs::canonicalize(&self.projects_dir)
            .map_err(|_| AppError::Internal("Projects directory not found".into()))?;
        if !canonical_dir.starts_with(&canonical_projects) {
            return Err(AppError::InvalidInput(
                "Memory directory escapes projects root".into(),
            ));
        }
        Ok(canonical_dir)
    }

    /// Validate file name and resolve to absolute path.
    /// Canonicalizes both dir and resolved path to prevent symlink traversal.
    /// Also verifies the canonical memory dir stays within the projects root.
    fn safe_file_path_buf(&self, project_id: &str, file_name: &str) -> Result<PathBuf, AppError> {
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
        let canonical_dir = self.safe_dir_path_buf(project_id)?;
        let resolved = self.dir_path_buf(project_id).join(trimmed);
        let canonical = std::fs::canonicalize(&resolved)
            .map_err(|_| AppError::NotFound(format!("File not found: {file_name}")))?;
        if !canonical.starts_with(&canonical_dir) {
            return Err(AppError::InvalidInput(format!(
                "Path escapes memory directory: {file_name}"
            )));
        }
        Ok(canonical)
    }
}

#[async_trait]
impl MemoryService for MemoryServiceImpl {
    async fn has_memory(&self, project_id: &str) -> Result<bool, AppError> {
        let dir = self.safe_dir_path_buf(project_id)?;
        let found = tokio::task::spawn_blocking(move || {
            let dir_path = std::path::Path::new(&dir);
            if !dir_path.exists() {
                return Ok::<bool, std::io::Error>(false);
            }
            let mut rd = match std::fs::read_dir(dir_path) {
                Ok(rd) => rd,
                Err(_) => return Ok::<bool, std::io::Error>(false),
            };
            while let Some(entry) = rd.next().transpose().ok().flatten() {
                if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                    if entry.file_name().to_string_lossy().to_lowercase().ends_with(".md") {
                        return Ok::<bool, std::io::Error>(true);
                    }
                }
            }
            Ok::<bool, std::io::Error>(false)
        })
        .await??;
        Ok(found)
    }

    async fn read_index(&self, project_id: &str) -> Result<Option<MemoryIndex>, AppError> {
        let dir = self.safe_dir_path_buf(project_id)?;
        let index = tokio::task::spawn_blocking(move || {
            let dir_path = std::path::Path::new(&dir);
            if !dir_path.exists() {
                return Ok::<Option<MemoryIndex>, std::io::Error>(None);
            }
            let mut names = Vec::new();
            let rd = match std::fs::read_dir(dir_path) {
                Ok(rd) => rd,
                Err(_) => return Ok::<Option<MemoryIndex>, std::io::Error>(None),
            };
            for entry in rd.flatten() {
                if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                    names.push(entry.file_name().to_string_lossy().to_string());
                }
            }
            let index_path = dir_path.join(INDEX_FILE_NAME);
            let raw = if index_path.exists() {
                let f = std::fs::File::open(&index_path)?;
                let meta = f.metadata()?;
                if meta.len() > MAX_FILE_SIZE {
                    return Ok::<Option<MemoryIndex>, std::io::Error>(Some(
                        parse_memory_index("", &names),
                    ));
                }
                let mut buf = String::with_capacity(meta.len() as usize);
                std::io::BufReader::new(f).read_to_string(&mut buf)?;
                buf
            } else {
                String::new()
            };
            Ok::<Option<MemoryIndex>, std::io::Error>(Some(parse_memory_index(&raw, &names)))
        })
        .await??;
        Ok(index)
    }

    async fn read_file(&self, project_id: &str, file_name: &str) -> Result<MemoryFile, AppError> {
        let canonical = self.safe_file_path_buf(project_id, file_name)?;
        let file_name_owned = file_name.to_string();
        let abs_path = canonical.to_string_lossy().to_string();
        let content = tokio::task::spawn_blocking(move || {
            // Open file handle first to prevent TOCTOU race — metadata
            // and read are done on the same handle, so symlink swap
            // between checks cannot bypass size limit or containment.
            let f = std::fs::File::open(&canonical)?;
            let meta = f.metadata()?;
            if meta.len() > MAX_FILE_SIZE {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Memory file exceeds {} KB limit", MAX_FILE_SIZE / 1024),
                ));
            }
            let mut content = String::new();
            std::io::BufReader::new(f).read_to_string(&mut content)?;
            Ok::<String, std::io::Error>(content)
        })
        .await??;
        Ok(MemoryFile {
            file_name: file_name_owned,
            absolute_path: abs_path,
            content,
        })
    }

    fn get_dir_path(&self, project_id: &str) -> String {
        self.dir_path_buf(project_id).to_string_lossy().to_string()
    }

    fn get_file_path(&self, project_id: &str, file_name: &str) -> Result<String, AppError> {
        self.safe_file_path_buf(project_id, file_name)
            .map(|p| p.to_string_lossy().to_string())
    }
}
