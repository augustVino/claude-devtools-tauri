//! Tauri IPC commands for Memory Viewer.
//!
//! Thin adapter: input validation + error wrapping, matching subagents.rs pattern.

use std::process::Stdio;
use std::sync::Arc;
use tauri::{command, State};
use tokio::io::AsyncWriteExt;

use crate::commands::guards;
use crate::error::AppError;
use crate::services::MemoryService;
use crate::types::memory::{MemoryIndex, MemoryOpenResult, MemoryReadFileResult, OpenTarget};

#[command]
pub async fn has_memory(
    service: State<'_, Arc<dyn MemoryService>>,
    project_id: String,
) -> Result<bool, String> {
    let safe_id = guards::validate_project_id(&project_id).map_err(|e| {
        log::error!("Invalid projectId: {e}");
        e
    })?;
    service
        .has_memory(&safe_id)
        .await
        .map_err(|e: AppError| e.into_tauri_string())
}

#[command]
pub async fn get_memory_index(
    service: State<'_, Arc<dyn MemoryService>>,
    project_id: String,
) -> Result<Option<MemoryIndex>, String> {
    let safe_id = guards::validate_project_id(&project_id).map_err(|e| {
        log::error!("Invalid projectId: {e}");
        e
    })?;
    service
        .read_index(&safe_id)
        .await
        .map_err(|e: AppError| e.into_tauri_string())
}

#[command]
pub async fn read_memory_file(
    service: State<'_, Arc<dyn MemoryService>>,
    project_id: String,
    file_name: String,
) -> Result<MemoryReadFileResult, String> {
    let safe_id = guards::validate_project_id(&project_id).map_err(|e| {
        log::error!("Invalid projectId: {e}");
        e
    })?;
    let safe_name = guards::validate_memory_file_name(&file_name).map_err(|e| {
        log::error!("Invalid fileName: {e}");
        e
    })?;
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

/// Write text to the system clipboard by piping to a subprocess.
async fn pipe_to_stdin(program: &str, args: &[&str], text: &str) -> Result<(), String> {
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start {program}: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .await
            .map_err(|e| format!("{program} write error: {e}"))?;
    }
    child
        .wait()
        .await
        .map_err(|e| format!("{program} failed: {e}"))?;
    Ok(())
}

/// macOS: pbcopy
#[cfg(target_os = "macos")]
async fn write_clipboard(text: &str) -> Result<(), String> {
    pipe_to_stdin("pbcopy", &[], text).await
}

/// Linux: wl-copy with xclip fallback
#[cfg(target_os = "linux")]
async fn write_clipboard(text: &str) -> Result<(), String> {
    if pipe_to_stdin("wl-copy", &[], text).await.is_ok() {
        return Ok(());
    }
    pipe_to_stdin("xclip", &["-selection", "clipboard"], text).await
}

/// Windows: clip
#[cfg(target_os = "windows")]
async fn write_clipboard(text: &str) -> Result<(), String> {
    pipe_to_stdin("clip", &[], text).await
}

#[command]
pub async fn copy_memory_path(
    service: State<'_, Arc<dyn MemoryService>>,
    project_id: String,
    file_name: Option<String>,
) -> Result<MemoryOpenResult, String> {
    let safe_id = guards::validate_project_id(&project_id).map_err(|e| {
        log::error!("Invalid projectId: {e}");
        e
    })?;
    let path = match file_name {
        Some(name) if !name.trim().is_empty() => {
            let safe_name = guards::validate_memory_file_name(&name).map_err(|e| {
                log::error!("Invalid fileName: {e}");
                e
            })?;
            service
                .get_file_path(&safe_id, &safe_name)
                .map_err(|e: AppError| e.into_tauri_string())?
        }
        _ => service.get_dir_path(&safe_id),
    };
    write_clipboard(&path).await?;
    Ok(MemoryOpenResult {
        success: true,
        path: Some(path),
        error: None,
    })
}

/// 列出已安装的外部应用，用于 Memory "Open in..." 菜单。
#[command]
pub async fn list_memory_openers() -> Result<Vec<OpenTarget>, String> {
    Ok(crate::utils::app_opener::detect_installations().await)
}

/// 用指定应用打开 Memory 文件或目录。
#[command]
pub async fn open_memory_in(
    service: State<'_, Arc<dyn MemoryService>>,
    project_id: String,
    file_name: Option<String>,
    opener_id: String,
) -> Result<MemoryOpenResult, String> {
    let safe_id = guards::validate_project_id(&project_id).map_err(|e| {
        log::error!("Invalid projectId: {e}");
        e
    })?;
    let safe_name = file_name
        .as_ref()
        .filter(|n| !n.trim().is_empty())
        .map(|n| guards::validate_memory_file_name(n))
        .transpose()
        .map_err(|e| {
            log::error!("Invalid fileName: {e}");
            e
        })?;

    match service
        .open_in(&opener_id, &safe_id, safe_name.as_deref())
        .await
    {
        Ok(()) => Ok(MemoryOpenResult {
            success: true,
            path: None,
            error: None,
        }),
        Err(e) => Ok(MemoryOpenResult {
            success: false,
            path: None,
            error: Some(e.to_string()),
        }),
    }
}
