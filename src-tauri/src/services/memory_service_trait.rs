//! Memory Service trait — per-project Claude memory directory reader.

use crate::error::AppError;
use crate::types::memory::{MemoryFile, MemoryIndex};
use async_trait::async_trait;

#[async_trait]
pub trait MemoryService: Send + Sync {
    async fn has_memory(&self, project_id: &str) -> Result<bool, AppError>;
    async fn read_index(&self, project_id: &str) -> Result<Option<MemoryIndex>, AppError>;
    async fn read_file(&self, project_id: &str, file_name: &str) -> Result<MemoryFile, AppError>;
    fn get_dir_path(&self, project_id: &str) -> String;
    fn get_file_path(&self, project_id: &str, file_name: &str) -> Result<String, AppError>;
    async fn open_in(
        &self,
        opener_id: &str,
        project_id: &str,
        file_name: Option<&str>,
    ) -> Result<(), AppError>;
}
