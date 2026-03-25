use std::collections::HashSet;

use tauri::{command, State};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::discovery::ProjectScanner;
use crate::infrastructure::{DataCache, ConfigManager};
use crate::parsing::{parse_session_file, ParsedSession};
use crate::types::domain::{Session, SessionMetrics, PaginatedSessionsResult};
use crate::types::chunks::SessionDetail;
use crate::utils::content_sanitizer::{
    extract_command_display, sanitize_display_content, is_command_output_content, is_command_content,
};
use crate::utils::{decode_path, extract_base_dir, extract_project_name, get_projects_base_path};
use crate::analysis::ChunkBuilder;

/// Application state shared across commands
pub struct AppState {
    pub cache: DataCache,
    pub config_manager: ConfigManager,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            cache: DataCache::new(),
            config_manager: ConfigManager::new(),
        }
    }

    pub async fn initialize(&self) -> Result<(), String> {
        self.config_manager.initialize().await
    }
}

// =============================================================================
// Session Commands
// =============================================================================

#[command]
pub async fn get_sessions(project_id: String) -> Result<Vec<Session>, String> {
    let base_path = get_projects_base_path();
    let project_dir_name = extract_base_dir(&project_id);
    let project_dir = base_path.join(&project_dir_name);

    if !project_dir.exists() {
        return Ok(vec![]);
    }

    let mut sessions = vec![];
    let mut entries = tokio::fs::read_dir(&project_dir)
        .await
        .map_err(|e| e.to_string())?;

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
            if let Some(session) = build_session_metadata(&path, &project_id).await {
                sessions.push(session);
            }
        }
    }

    // Sort by created_at descending
    sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    Ok(sessions)
}

#[command]
pub async fn get_session_detail(
    state: State<'_, Arc<RwLock<AppState>>>,
    project_id: String,
    session_id: String,
) -> Result<Option<SessionDetail>, String> {
    let cache_key = format!("{}/{}", project_id, session_id);

    // Check cache first
    let app_state = state.read().await;
    if let Some(cached) = app_state.cache.get_session(&project_id, &session_id).await {
        if let Ok(detail) = serde_json::from_value(cached) {
            return Ok(Some(detail));
        }
    }
    drop(app_state);

    // Parse session file
    let base_path = get_projects_base_path();
    let project_dir = extract_base_dir(&project_id);
    let session_path = base_path
        .join(&project_dir)
        .join(format!("{}.jsonl", session_id));

    if !session_path.exists() {
        return Ok(None);
    }

    let parsed = parse_session_file(&session_path).await;

    let session = build_session_metadata(&session_path, &project_id).await.unwrap_or_else(|| Session {
        id: session_id.clone(),
        project_id: project_id.clone(),
        project_path: decode_path(&project_id),
        created_at: 0,
        todo_data: None,
        first_message: None,
        message_timestamp: None,
        has_subagents: !parsed.task_calls.is_empty(),
        message_count: parsed.messages.len() as u32,
        is_ongoing: None,
        git_branch: None,
        metadata_level: None,
        context_consumption: None,
        compaction_count: None,
        phase_breakdown: None,
    });

    // Build chunks from parsed messages using ChunkBuilder
    let detail = ChunkBuilder::build_session_detail(
        session,
        parsed.messages.clone(),
        vec![], // subagents populated in Phase 4
    );

    // Cache the result
    let app_state = state.read().await;
    app_state.cache.set_session(&project_id, &session_id, serde_json::to_value(&detail).unwrap_or_default()).await;

    Ok(Some(detail))
}

#[command]
pub async fn get_session_metrics(
    project_id: String,
    session_id: String,
) -> Result<Option<SessionMetrics>, String> {
    let base_path = get_projects_base_path();
    let project_dir = extract_base_dir(&project_id);
    let session_path = base_path
        .join(&project_dir)
        .join(format!("{}.jsonl", session_id));

    if !session_path.exists() {
        return Ok(None);
    }

    let parsed = parse_session_file(&session_path).await;
    Ok(Some(parsed.metrics))
}

// =============================================================================
// Project Commands
// =============================================================================

#[command]
pub async fn get_projects() -> Result<Vec<crate::types::domain::Project>, String> {
    let base_path = get_projects_base_path();

    if !base_path.exists() {
        return Ok(vec![]);
    }

    let mut projects = vec![];
    let mut entries = tokio::fs::read_dir(&base_path)
        .await
        .map_err(|e| e.to_string())?;

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.is_dir() {
            let project_id = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let project_path = decode_path(&project_id);
            let name = extract_project_name(&project_id, None);

            // Count sessions
            let session_count = count_sessions_in_dir(&path).await;

            projects.push(crate::types::domain::Project {
                id: project_id,
                path: project_path,
                name,
                sessions: vec![], // Will be loaded on demand
                created_at: path.metadata()
                    .and_then(|m| m.created())
                    .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64)
                    .unwrap_or(0),
                most_recent_session: None,
            });
        }
    }

    // Sort by most recent session (placeholder for now)
    projects.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    Ok(projects)
}

// =============================================================================
// Session Pagination Commands
// =============================================================================

/// Get paginated sessions for a project.
#[command]
pub async fn get_sessions_paginated(
    project_id: String,
    cursor: Option<String>,
    limit: Option<u32>,
) -> Result<PaginatedSessionsResult, String> {
    let page_limit = limit.unwrap_or(50).min(100).max(1) as usize;

    let scanner = ProjectScanner::new();
    let all_sessions = scanner.list_sessions(&project_id);

    let total_count = all_sessions.len() as u32;

    // Find cursor position
    let start_idx = if let Some(ref c) = cursor {
        all_sessions.iter().position(|s| &s.id == c).unwrap_or(0)
    } else {
        0
    };

    // Get page slice
    let end_idx = (start_idx + page_limit).min(all_sessions.len());
    let sessions = all_sessions[start_idx..end_idx].to_vec();

    let has_more = end_idx < all_sessions.len();
    let next_cursor = if has_more {
        sessions.last().map(|s| s.id.clone())
    } else {
        None
    };

    Ok(PaginatedSessionsResult {
        sessions,
        next_cursor,
        has_more,
        total_count,
    })
}

/// Get sessions by their IDs.
#[command]
pub async fn get_sessions_by_ids(
    project_id: String,
    session_ids: Vec<String>,
) -> Result<Vec<Session>, String> {
    let id_set: HashSet<String> = session_ids.into_iter().collect();

    if id_set.is_empty() {
        return Ok(Vec::new());
    }

    let scanner = ProjectScanner::new();
    let all_sessions = scanner.list_sessions(&project_id);

    Ok(all_sessions
        .into_iter()
        .filter(|s| id_set.contains(&s.id))
        .collect())
}

// =============================================================================
// Helper Functions
// =============================================================================

async fn build_session_metadata(path: &std::path::Path, project_id: &str) -> Option<Session> {
    let filename = path.file_stem()?.to_string_lossy().to_string();
    let metadata = path.metadata().ok()?;

    let parsed = parse_session_file(path).await;

    // Extract first user message as title (aligned with Electron's analyzeSessionFileMetadata)
    let mut first_user_text: Option<String> = None;
    let mut first_command_text: Option<String> = None;

    for msg in &parsed.messages {
        if msg.message_type != crate::types::domain::MessageType::User {
            continue;
        }
        if msg.is_meta {
            continue;
        }

        let text = match &msg.content {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Array(arr) => arr
                .iter()
                .filter_map(|block| {
                    if block.get("type")?.as_str()? == "text" {
                        block.get("text")?.as_str().map(|s| s.trim().to_string())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
            _ => continue,
        };

        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Skip command output and interruptions
        if is_command_output_content(trimmed)
            || trimmed.starts_with("[Request interrupted by user")
        {
            continue;
        }

        // Store command-name as fallback, keep looking for real text
        if is_command_content(trimmed) {
            if first_command_text.is_none() {
                first_command_text = extract_command_display(trimmed);
            }
            continue;
        }

        // Real user text found — sanitize and truncate to 500 chars
        let sanitized = sanitize_display_content(trimmed);
        if sanitized.is_empty() {
            continue;
        }
        first_user_text = Some(sanitized.chars().take(500).collect());
        break;
    }

    let first_message = first_user_text.or(first_command_text);

    Some(Session {
        id: filename,
        project_id: project_id.to_string(),
        project_path: decode_path(project_id),
        created_at: metadata.created()
            .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64)
            .unwrap_or(0),
        todo_data: None,
        first_message,
        message_timestamp: None,
        has_subagents: !parsed.task_calls.is_empty(),
        message_count: parsed.messages.len() as u32,
        is_ongoing: None,
        git_branch: parsed.messages.first().and_then(|m| m.git_branch.clone()),
        metadata_level: None,
        context_consumption: None,
        compaction_count: None,
        phase_breakdown: None,
    })
}

async fn count_sessions_in_dir(dir: &std::path::Path) -> u32 {
    let mut count = 0u32;
    if let Ok(mut entries) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.path().extension().map(|e| e == "jsonl").unwrap_or(false) {
                count += 1;
            }
        }
    }
    count
}