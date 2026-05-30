//! Tauri IPC commands for Memory Viewer.
//!
//! Thin adapter: input validation + error wrapping, matching subagents.rs pattern.

use std::sync::Arc;
use tauri::{command, State};

use crate::commands::guards;
use crate::error::AppError;
use crate::services::MemoryService;
use crate::types::memory::{MemoryIndex, MemoryReadFileResult, MemoryOpenResult};

#[command]
pub async fn has_memory(
    service: State<'_, Arc<dyn MemoryService>>,
    project_id: String,
) -> Result<bool, String> {
    let safe_id = guards::validate_project_id(&project_id)
        .map_err(|e| { log::error!("Invalid projectId: {e}"); e })?;
    service.has_memory(&safe_id)
        .await
        .map_err(|e: AppError| e.into_tauri_string())
}

#[command]
pub async fn get_memory_index(
    service: State<'_, Arc<dyn MemoryService>>,
    project_id: String,
) -> Result<Option<MemoryIndex>, String> {
    let safe_id = guards::validate_project_id(&project_id)
        .map_err(|e| { log::error!("Invalid projectId: {e}"); e })?;
    service.read_index(&safe_id)
        .await
        .map_err(|e: AppError| e.into_tauri_string())
}

#[command]
pub async fn read_memory_file(
    service: State<'_, Arc<dyn MemoryService>>,
    project_id: String,
    file_name: String,
) -> Result<MemoryReadFileResult, String> {
    let safe_id = guards::validate_project_id(&project_id)
        .map_err(|e| { log::error!("Invalid projectId: {e}"); e })?;
    let safe_name = guards::validate_memory_file_name(&file_name)
        .map_err(|e| { log::error!("Invalid fileName: {e}"); e })?;
    match service.read_file(&safe_id, &safe_name).await {
        Ok(file) => Ok(MemoryReadFileResult {
            success: true,
            content: Some(file.content),
            path: Some(file.absolute_path),
            error: None,
        }),
        Err(e) => {
            log::error!("Error reading memory file: {e}");
            Ok(MemoryReadFileResult {
                success: false,
                content: None,
                path: None,
                error: Some("Failed to read file".into()),
            })
        }
    }
}

#[command]
pub async fn copy_memory_path(
    service: State<'_, Arc<dyn MemoryService>>,
    project_id: String,
    file_name: Option<String>,
) -> Result<MemoryOpenResult, String> {
    let safe_id = guards::validate_project_id(&project_id)
        .map_err(|e| { log::error!("Invalid projectId: {e}"); e })?;
    let path = match file_name {
        Some(name) if !name.trim().is_empty() => {
            let safe_name = guards::validate_memory_file_name(&name)
                .map_err(|e| { log::error!("Invalid fileName: {e}"); e })?;
            service.get_file_path(&safe_id, &safe_name)
                .map_err(|e: AppError| e.into_tauri_string())?
        }
        _ => service.get_dir_path(&safe_id),
    };
    Ok(MemoryOpenResult {
        success: true,
        path: Some(path),
        error: None,
    })
}
