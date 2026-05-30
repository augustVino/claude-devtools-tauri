//! Memory Service implementation — reads from local filesystem.

use async_trait::async_trait;
use std::path::PathBuf;
use std::io::Read;
use crate::error::AppError;
use crate::types::memory::{MemoryFile, MemoryIndex, parse_memory_index};
use crate::utils::path_decoder;
use crate::utils::app_opener;

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
    /// Uses the encoded directory name directly — do NOT decode_path, as
    /// that loses information (hyphens in real directory names get
    /// converted to `/`, producing wrong paths).
    fn dir_path_buf(&self, project_id: &str) -> PathBuf {
        let base_dir = path_decoder::extract_base_dir(project_id);
        self.projects_dir.join(base_dir).join(MEMORY_DIR_NAME)
    }

    /// Validate memory directory path via canonicalize + containment check.
    /// Used by `safe_file_path_buf` and as a model for the re-canonicalize
    /// logic inside `has_memory` and `read_index` spawn_blocking closures.
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

    /// Re-canonicalize a resolved directory path, checking containment.
    /// Called inside spawn_blocking closures to close TOCTOU window between
    /// pre-closure canonicalization and actual filesystem access.
    fn re_canonicalize_dir(
        dir: &std::path::Path,
        projects_dir: &std::path::Path,
    ) -> Result<std::path::PathBuf, std::io::Error> {
        let canonical_dir = std::fs::canonicalize(dir)?;
        let canonical_projects = std::fs::canonicalize(projects_dir)?;
        if !canonical_dir.starts_with(&canonical_projects) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Memory directory escapes projects root",
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
        let dir = self.dir_path_buf(project_id);
        let projects = self.projects_dir.clone();
        let found = tokio::task::spawn_blocking(move || {
            // Re-canonicalize inside the closure to prevent TOCTOU race:
            // the directory path could be swapped between pre-check and read.
            let canonical_dir = match Self::re_canonicalize_dir(&dir, &projects) {
                Ok(d) => d,
                Err(_) => return Ok::<bool, std::io::Error>(false),
            };
            if !canonical_dir.exists() {
                return Ok::<bool, std::io::Error>(false);
            }
            let mut rd = match std::fs::read_dir(&canonical_dir) {
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
        let dir = self.dir_path_buf(project_id);
        let projects = self.projects_dir.clone();
        let index = tokio::task::spawn_blocking(move || {
            // Re-canonicalize inside the closure to prevent TOCTOU race.
            let canonical_dir = match Self::re_canonicalize_dir(&dir, &projects) {
                Ok(d) => d,
                Err(_) => return Ok::<Option<MemoryIndex>, std::io::Error>(None),
            };
            let mut names = Vec::new();
            let rd = match std::fs::read_dir(&canonical_dir) {
                Ok(rd) => rd,
                Err(_) => return Ok::<Option<MemoryIndex>, std::io::Error>(None),
            };
            for entry in rd.flatten() {
                if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                    names.push(entry.file_name().to_string_lossy().to_string());
                }
            }
            let index_path = canonical_dir.join(INDEX_FILE_NAME);
            let raw = if index_path.exists() {
                let canonical_index = std::fs::canonicalize(&index_path)
                    .map_err(|_| std::io::Error::new(std::io::ErrorKind::NotFound, "Index file not found"))?;
                if !canonical_index.starts_with(&canonical_dir) {
                    return Ok::<Option<MemoryIndex>, std::io::Error>(Some(
                        parse_memory_index("", &names),
                    ));
                }
                let f = std::fs::File::open(&canonical_index)?;
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
        let file_name_owned = file_name.to_string();
        let dir = self.dir_path_buf(project_id);
        let projects = self.projects_dir.clone();
        let trimmed = file_name.trim().to_string();

        // Validate file name syntax
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

        let content = tokio::task::spawn_blocking(move || {
            // Canonicalize inside the closure to prevent TOCTOU race:
            // resolve dir, verify containment, resolve file, verify containment, then read.
            let canonical_dir = Self::re_canonicalize_dir(&dir, &projects)?;
            let resolved = canonical_dir.join(&trimmed);
            let canonical = std::fs::canonicalize(&resolved)
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::NotFound, format!("File not found: {trimmed}")))?;
            if !canonical.starts_with(&canonical_dir) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Path escapes memory directory: {trimmed}"),
                ));
            }
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
            Ok::<(String, String), std::io::Error>((content, canonical.to_string_lossy().to_string()))
        })
        .await??;
        Ok(MemoryFile {
            file_name: file_name_owned,
            absolute_path: content.1,
            content: content.0,
        })
    }

    fn get_dir_path(&self, project_id: &str) -> String {
        self.dir_path_buf(project_id).to_string_lossy().to_string()
    }

    fn get_file_path(&self, project_id: &str, file_name: &str) -> Result<String, AppError> {
        self.safe_file_path_buf(project_id, file_name)
            .map(|p| p.to_string_lossy().to_string())
    }

    async fn open_in(
        &self,
        opener_id: &str,
        project_id: &str,
        file_name: Option<&str>,
    ) -> Result<(), AppError> {
        let path = match file_name {
            Some(name) if !name.trim().is_empty() => {
                self.get_file_path(project_id, name)?
            }
            _ => self.get_dir_path(project_id),
        };
        let is_directory = file_name.map_or(true, |n| n.trim().is_empty());
        app_opener::open_with(opener_id, &path, is_directory).await
    }
}
