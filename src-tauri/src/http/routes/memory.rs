//! HTTP route handlers for Memory Viewer.
//!
//! Uses `Result<_, AppError>` + `IntoResponse` pattern (matches subagents.rs/ssh.rs).
//! HTTP responses strip absolute paths (return only file names) to avoid
//! leaking server filesystem layout.

use axum::{
    extract::{Query, State},
    Json,
};
use serde::Deserialize;

use crate::commands::guards;
use crate::error::AppError;
use crate::http::state::HttpState;
use crate::types::memory::{MemoryIndex, MemoryOpenResult, MemoryReadFileResult, OpenTarget};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryQuery {
    pub project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryFileQuery {
    pub project_id: Option<String>,
    pub file: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyPathBody {
    pub project_id: Option<String>,
    pub file_name: Option<String>,
}

fn require_project_id(id: Option<String>) -> Result<String, AppError> {
    id.filter(|s| !s.trim().is_empty())
        .ok_or_else(|| AppError::InvalidInput("projectId is required".into()))
        .and_then(|s| guards::validate_project_id(&s).map_err(|e| AppError::InvalidInput(e)))
}

pub async fn has_memory(
    State(state): State<HttpState>,
    Query(query): Query<MemoryQuery>,
) -> Result<Json<bool>, AppError> {
    let safe_id = require_project_id(query.project_id)?;
    state.memory_svc.has_memory(&safe_id).await.map(Json)
}

pub async fn get_memory_index(
    State(state): State<HttpState>,
    Query(query): Query<MemoryQuery>,
) -> Result<Json<Option<MemoryIndex>>, AppError> {
    let safe_id = require_project_id(query.project_id)?;
    state.memory_svc.read_index(&safe_id).await.map(Json)
}

pub async fn read_memory_file(
    State(state): State<HttpState>,
    Query(query): Query<MemoryFileQuery>,
) -> Result<Json<MemoryReadFileResult>, AppError> {
    let safe_id = require_project_id(query.project_id)?;
    let file_name = query
        .file
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| AppError::InvalidInput("file is required".into()))?;
    let safe_name =
        guards::validate_memory_file_name(&file_name).map_err(|e| AppError::InvalidInput(e))?;

    match state.memory_svc.read_file(&safe_id, &safe_name).await {
        Ok(file) => Ok(Json(MemoryReadFileResult {
            success: true,
            content: Some(file.content),
            // HTTP mode: strip absolute path, return only file name
            path: Some(file.file_name),
            error: None,
        })),
        Err(e) => {
            log::error!("Error reading memory file via HTTP: {e}");
            Ok(Json(MemoryReadFileResult {
                success: false,
                content: None,
                path: None,
                // Sanitize: don't expose full filesystem paths to HTTP clients
                error: Some("Failed to read file".into()),
            }))
        }
    }
}

pub async fn copy_memory_path(
    State(state): State<HttpState>,
    Json(body): Json<CopyPathBody>,
) -> Result<Json<MemoryOpenResult>, AppError> {
    let safe_id = require_project_id(body.project_id)?;
    // HTTP mode: return only the file name, not absolute path,
    // to avoid leaking server filesystem layout.
    let path = match body.file_name {
        Some(name) if !name.trim().is_empty() => {
            let safe_name =
                guards::validate_memory_file_name(&name).map_err(|e| AppError::InvalidInput(e))?;
            safe_name
        }
        _ => "memory".to_string(),
    };
    Ok(Json(MemoryOpenResult {
        success: true,
        path: Some(path),
        error: None,
    }))
}

pub async fn list_memory_openers(
    _state: State<HttpState>,
) -> Result<Json<Vec<OpenTarget>>, AppError> {
    Ok(Json(crate::utils::app_opener::detect_installations().await))
}

pub async fn open_memory_in(_state: State<HttpState>) -> Result<Json<MemoryOpenResult>, AppError> {
    // HTTP 模式下不可用 — 对齐 upstream httpClient.ts 的 openIn 行为
    Ok(Json(MemoryOpenResult {
        success: false,
        path: None,
        error: Some("Open operations are not available in HTTP mode".into()),
    }))
}
