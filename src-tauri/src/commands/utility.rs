//! IPC Handlers for Utility Operations.
//!
//! Handlers:
//! - open_path: Open a path in the system file manager
//! - open_external: Open a URL in the system browser
//! - get_zoom_factor: Get the current zoom factor
//! - read_claude_md_files: Read all CLAUDE.md files for a project
//! - read_directory_claude_md: Read a specific directory's CLAUDE.md file
//! - read_mentioned_file: Read a mentioned file for context injection
//! - read_agent_configs: Read agent configurations from .claude/agents/
//! - write_text_file: Write text content to a file

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;

use crate::parsing::claude_md_reader::{ClaudeMdReader, ClaudeMdFileInfo};

/// Open a path in the system file manager.
///
/// 包含与 Electron 对齐的安全校验：
/// - 路径归一化（消除 traversal）
/// - 敏感文件模式匹配
/// - 目录白名单限制（~/.claude 和可选的项目目录）
/// - Symlink escape 防护
#[tauri::command]
pub async fn open_path(app: tauri::AppHandle, path: String, project_root: Option<String>) -> Result<(), String> {
    let expanded = if path.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            let remainder = path[1..].trim_start_matches('/');
            home.join(remainder)
        } else {
            std::path::PathBuf::from(&path)
        }
    } else {
        std::path::PathBuf::from(&path)
    };

    let p = expanded;

    // 必须是绝对路径
    if !p.is_absolute() {
        return Err("Path must be absolute".to_string());
    }

    // 归一化路径（消除 .. traversal）
    let normalized = normalize_path(&p);

    // 敏感文件模式匹配
    if matches_sensitive_pattern(&normalized) {
        return Err("Cannot open sensitive files".to_string());
    }

    // 目录白名单校验（~/.claude 和可选的项目目录）
    let project_normalized = project_root.as_ref().map(|root| normalize_path(&PathBuf::from(root)));
    if !is_within_allowed_directories(&normalized, project_normalized.as_deref()) {
        return Err("Path is outside allowed directories".to_string());
    }

    // Symlink escape 防护
    if let Some(real_path) = resolve_real_path(&p) {
        let real_normalized = normalize_path(&real_path);
        if matches_sensitive_pattern(&real_normalized) {
            return Err("Cannot open sensitive files".to_string());
        }
        if !is_within_allowed_directories(&real_normalized, project_normalized.as_deref()) {
            return Err("Path is outside allowed directories".to_string());
        }
    }

    // 检查路径是否存在
    if !p.exists() {
        return Err(format!("Path does not exist: {}", p.display()));
    }

    app.opener()
        .open_path(p.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| format!("Failed to open path: {}", e))?;
    Ok(())
}

/// 归一化路径，消除 `.` 和 `..` traversal 段。
fn normalize_path(p: &std::path::Path) -> std::path::PathBuf {
    let mut normalized = std::path::PathBuf::new();
    for component in p.components() {
        match component {
            std::path::Component::ParentDir => { normalized.pop(); }
            std::path::Component::CurDir => {}
            _ => normalized.push(component),
        }
    }
    normalized
}

/// 检查路径是否匹配敏感文件模式（与 Electron pathValidation.ts SENSITIVE_PATTERNS 对齐）。
///
/// 使用大小写不敏感的路径组件匹配，并对齐 Electron 正则的锚点语义：
/// - `contains` 模式：匹配路径中任意位置包含的目录/子串
/// - `ends_with_component` 模式：匹配路径最后一个组件以指定后缀结尾
/// - `exact` 模式：匹配完整路径精确相等
fn matches_sensitive_pattern(normalized: &std::path::Path) -> bool {
    let path_lower = normalized.to_string_lossy().to_lowercase();

    // 目录/子串包含匹配（对应 Electron 中不带 $ 锚点的正则）
    let contains_patterns: &[&str] = &[
        "/.ssh/",           // SSH keys and config
        "/.aws/",           // AWS credentials
        "/.config/gcloud/", // GCP credentials
        "/.azure/",         // Azure credentials
        "/.docker/config.json", // Docker credentials
        "/.kube/config",    // Kubernetes config
    ];
    if contains_patterns.iter().any(|pat| path_lower.contains(pat)) {
        return true;
    }

    // 文件名组件结尾匹配（对应 Electron 中带 $ 锚点的正则）
    // 检查路径最后一个 `/` 之后的组件是否以指定模式结尾
    let filename = normalized.file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let filename_str = filename.as_str();

    // .env 系列（对应 /[/\\]\.env($|\.)/i）
    if filename_str.starts_with(".env") {
        let rest = &filename_str[4..];
        if rest.is_empty() || rest.starts_with('.') {
            return true;
        }
    }

    // 精确文件名匹配（对应 Electron 带 $ 锚点的正则）
    let exact_names: &[&str] = &[
        ".git-credentials",
        ".gitconfig",
        ".npmrc",
        ".password",
        ".secret",
        "id_rsa",
        "id_ed25519",
        "id_ecdsa",
    ];
    if exact_names.iter().any(|name| filename_str == *name) {
        return true;
    }

    // 后缀匹配（对应 Electron /[/\\][^/\\]*\.ext$/i）
    // 要求最后一个组件以 .pem 或 .key 结尾（整个文件名，不是子串）
    if filename_str.ends_with(".pem") || filename_str.ends_with(".key") {
        return true;
    }

    // 精确路径匹配（对应 Electron /^\/etc\/passwd$/i 等）
    let path_str = normalized.to_string_lossy();
    if path_str == "/etc/passwd" || path_str == "/etc/shadow" {
        return true;
    }

    // 文件名结尾匹配（对应 Electron /credentials\.json$/i 等）
    if filename_str.ends_with("credentials.json")
        || filename_str.ends_with("secrets.json")
        || filename_str.ends_with("tokens.json")
    {
        return true;
    }

    false
}

/// 检查路径是否在允许的目录内（~/.claude 和可选的项目目录）。
fn is_within_allowed_directories(normalized: &std::path::Path, project_root: Option<&std::path::Path>) -> bool {
    // 始终允许 ~/.claude 目录（与 Electron 对齐）
    if let Some(home) = dirs::home_dir() {
        let claude_dir = home.join(".claude");
        let claude_normalized = normalize_path(&claude_dir);
        if path_starts_with(normalized, &claude_normalized) {
            return true;
        }
    }
    // 如果提供了项目目录，也允许访问
    if let Some(root) = project_root {
        if path_starts_with(normalized, root) {
            return true;
        }
    }
    false
}

/// 检查 target 是否在 root 下（或等于 root）。
fn path_starts_with(target: &std::path::Path, root: &std::path::Path) -> bool {
    target == root || target.starts_with(root)
}

/// 解析符号链接的真实路径（如果存在）。
fn resolve_real_path(p: &std::path::Path) -> Option<std::path::PathBuf> {
    p.canonicalize().ok()
}

/// Open a URL in the system browser.
/// Supports http, https, and mailto protocols (aligned with Electron).
#[tauri::command]
pub async fn open_external(app: tauri::AppHandle, url: String) -> Result<(), String> {
    // 与 Electron 对齐：允许 http、https、mailto 协议
    let allowed = url.starts_with("http://")
        || url.starts_with("https://")
        || url.starts_with("mailto:");
    if !allowed {
        return Err("Invalid URL: only http, https, and mailto URLs are allowed".to_string());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Zoom factor result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoomFactorResult {
    pub factor: f64,
}

/// Get the current zoom factor from app state.
/// Tauri v2 has set_zoom() but no zoom() getter, so we track it ourselves.
#[tauri::command]
pub async fn get_zoom_factor(
    zoom_state: State<'_, Arc<AtomicU64>>,
) -> Result<ZoomFactorResult, String> {
    let bits = zoom_state.load(Ordering::Relaxed);
    Ok(ZoomFactorResult {
        factor: f64::from_bits(bits),
    })
}

/// Set the zoom factor and persist it in app state.
/// Clamps to [0.5, 3.0] range (~50% to ~300%).
#[tauri::command]
pub async fn set_zoom_factor(
    app: AppHandle,
    zoom_state: State<'_, Arc<AtomicU64>>,
    factor: f64,
) -> Result<(), String> {
    let clamped = factor.clamp(0.5, 3.0);
    if let Some(window) = app.get_webview_window("main") {
        window
            .set_zoom(clamped)
            .map_err(|e| e.to_string())?;
    }
    zoom_state.store(clamped.to_bits(), Ordering::Relaxed);
    Ok(())
}

// =============================================================================
// CLAUDE.md Commands (synchronous methods)
// =============================================================================

/// Read all CLAUDE.md files for a project.
/// Note: ClaudeMdReader methods are synchronous.
/// Returns flat HashMap to match Electron IPC (which unwraps ClaudeMdReadResult.files).
#[tauri::command]
pub fn read_claude_md_files(
    project_root: String,
) -> std::collections::HashMap<String, ClaudeMdFileInfo> {
    let reader = ClaudeMdReader::new();
    reader.read_all_claude_md_files(&project_root).files
}

/// Read a specific directory's CLAUDE.md file.
#[tauri::command]
pub fn read_directory_claude_md(directory: String) -> ClaudeMdFileInfo {
    let reader = ClaudeMdReader::new();
    reader.read_directory_claude_md(&directory)
}

/// Mentioned file info for context injection (matches Electron MentionedFileInfo).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionedFileInfo {
    pub path: String,
    pub exists: bool,
    pub char_count: usize,
    pub estimated_tokens: usize,
}

/// Read a mentioned file for context injection.
/// Returns MentionedFileInfo with token count (matches Electron HTTP API behavior).
#[tauri::command]
pub async fn read_mentioned_file(
    file_path: String,
    _project_root: String,
    max_tokens: Option<usize>,
) -> Result<Option<MentionedFileInfo>, String> {
    let max_tokens_limit = max_tokens.unwrap_or(25000);
    let path = Path::new(&file_path);

    // Skip non-existent paths and directories
    if !path.exists() || path.is_dir() {
        return Ok(None);
    }

    match tokio::fs::read_to_string(path).await {
        Ok(content) => {
            if content.len() > 1_000_000 {
                return Ok(None);
            }

            let char_count = content.len();
            // Simple token estimation: ~4 chars per token
            let estimated_tokens = char_count / 4;

            if estimated_tokens > max_tokens_limit {
                return Ok(None);
            }

            Ok(Some(MentionedFileInfo {
                path: file_path,
                exists: true,
                char_count,
                estimated_tokens,
            }))
        }
        Err(_) => Ok(None),
    }
}

/// Agent config for IPC (matches frontend AgentConfig interface).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    pub name: String,
    pub color: Option<String>,
}

/// Read agent configurations from .claude/agents/ directory.
/// Returns a map from agent name to AgentConfig (matches Electron HTTP API behavior).
#[tauri::command]
pub fn read_agent_configs(project_root: String) -> std::collections::HashMap<String, AgentConfig> {
    let configs = crate::parsing::agent_config_reader::read_agent_configs(&project_root);
    configs
        .into_iter()
        .map(|(name, config)| {
            (
                name.clone(),
                AgentConfig {
                    name,
                    color: config.color,
                },
            )
        })
        .collect()
}

/// Write text content to a file at the given path.
/// Used by the export flow after the user picks a save location via the native dialog.
#[tauri::command]
pub async fn write_text_file(path: String, content: String) -> Result<(), String> {
    tokio::fs::write(&path, content)
        .await
        .map_err(|e| format!("Failed to write file: {}", e))
}

#[cfg(test)]
mod write_text_file_tests {
    use super::*;

    #[tokio::test]
    async fn test_write_text_file_creates_file() {
        let dir = tempfile::TempDir::new().expect("temp dir creation");
        let path = dir.path().join("test-export.md");
        let content = "# Hello\nThis is a test.".to_string();

        write_text_file(path.to_string_lossy().to_string(), content.clone())
            .await
            .expect("write should succeed");

        let read_back = tokio::fs::read_to_string(&path).await.expect("should read back");
        assert_eq!(read_back, content);
    }

    #[tokio::test]
    async fn test_write_text_file_overwrites_existing() {
        let dir = tempfile::TempDir::new().expect("temp dir creation");
        let path = dir.path().join("test-overwrite.md");

        write_text_file(path.to_string_lossy().to_string(), "old content".to_string())
            .await
            .expect("first write should succeed");

        write_text_file(path.to_string_lossy().to_string(), "new content".to_string())
            .await
            .expect("overwrite should succeed");

        let read_back = tokio::fs::read_to_string(&path).await.expect("should read back");
        assert_eq!(read_back, "new content");
    }

    #[tokio::test]
    async fn test_write_text_file_nonexistent_directory() {
        let result = write_text_file(
            "/nonexistent/tauri_test_dir/file.txt".to_string(),
            "content".to_string(),
        )
        .await;

        assert!(result.is_err(), "expected error for nonexistent directory");
        let err_msg = result.unwrap_err();
        assert!(
            err_msg.contains("Failed to write file"),
            "error message should contain 'Failed to write file', got: {}",
            err_msg
        );
    }
}