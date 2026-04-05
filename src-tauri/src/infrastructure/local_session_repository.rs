//! 本地文件系统的 SessionRepository 实现。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use log::{info, warn};

use crate::error::AppError;
use crate::infrastructure::fs_provider::{FsProvider, FsStatResult};
use crate::infrastructure::session_repository::{DeleteFilesResult, SessionFileItem, SessionRepository};
use crate::parsing::{parse_session_file, ParsedSession};
use crate::utils::path_decoder::extract_base_dir;

/// 本地会话 Repository — 基于 FsProvider 访问本地文件系统。
pub struct LocalSessionRepository {
    #[allow(dead_code)]
    fs_provider: Arc<dyn FsProvider>,
    #[allow(dead_code)]
    projects_dir: PathBuf,
    #[allow(dead_code)]
    claude_base_dir: PathBuf,
}

impl LocalSessionRepository {
    pub fn new(
        fs_provider: Arc<dyn FsProvider>,
        projects_dir: PathBuf,
        claude_base_dir: PathBuf,
    ) -> Self {
        Self {
            fs_provider,
            projects_dir,
            claude_base_dir,
        }
    }

    #[allow(dead_code)]
    fn project_dir(&self, project_id: &str) -> PathBuf {
        let name = extract_base_dir(project_id);
        self.projects_dir.join(&name)
    }
}

#[async_trait]
impl SessionRepository for LocalSessionRepository {
    async fn read_raw_session(
        &self,
        project_id: &str,
        session_file: &str,
    ) -> Result<ParsedSession, AppError> {
        let path = self.project_dir(project_id).join(session_file);
        if !path.exists() {
            return Err(AppError::NotFound(format!(
                "Session file not found: {}",
                path.display()
            )));
        }
        Ok(parse_session_file(&path).await)
    }

    async fn session_exists(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<bool, AppError> {
        let path = self.project_dir(project_id).join(format!("{}.jsonl", session_id));
        Ok(path.exists())
    }

    async fn session_stat(&self, path: &Path) -> Result<FsStatResult, AppError> {
        self.fs_provider
            .stat(path)
            .map_err(|e| AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))
    }

    async fn list_session_files(&self, project_id: &str) -> Result<Vec<SessionFileItem>, AppError> {
        let project_dir = self.project_dir(project_id);

        if !self
            .fs_provider
            .exists(&project_dir)
            .map_err(|e| AppError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?
        {
            return Ok(Vec::new());
        }

        let mut entries = tokio::fs::read_dir(&project_dir)
            .await
            .map_err(|e| AppError::Io(e))?;

        let mut files = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().map(|e| e == "jsonl").unwrap_or(false) {
                let session_id = path
                    .file_stem()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let mtime_ms = entry
                    .metadata()
                    .await
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                files.push(SessionFileItem {
                    path,
                    session_id,
                    mtime_ms,
                });
            }
        }

        // 按 mtime 降序排列（与 Electron 一致）
        files.sort_by(|a, b| b.mtime_ms.cmp(&a.mtime_ms));
        Ok(files)
    }

    async fn delete_session_files(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Result<DeleteFilesResult, AppError> {
        // Validate UUID
        if uuid::Uuid::parse_str(session_id).is_err() {
            return Err(AppError::InvalidInput(format!(
                "Invalid session_id: '{}'",
                session_id
            )));
        }

        let project_dir = self.project_dir(project_id);
        let claude_base = &self.claude_base_dir;
        let mut associated_deleted = 0u32;

        async fn try_remove_file(path: &Path) -> bool {
            tokio::fs::remove_file(path).await.is_ok()
        }
        async fn try_remove_dir(path: &Path) -> bool {
            tokio::fs::remove_dir_all(path).await.is_ok()
        }

        // 1. Main JSONL file
        let jsonl_path = project_dir.join(format!("{}.jsonl", session_id));
        if jsonl_path.exists() {
            if try_remove_file(&jsonl_path).await {
                info!("Deleted session file: {}", jsonl_path.display());
            } else {
                warn!("Failed to delete session file: {}", jsonl_path.display());
            }
        }

        // 2. Session directory (subagents + tool-results)
        let session_dir = project_dir.join(session_id);
        if session_dir.exists() {
            if try_remove_dir(&session_dir).await {
                associated_deleted += 1;
                info!("Deleted session directory: {}", session_dir.display());
            }
        }

        // 3. file-history
        let fh_dir = claude_base.join("file-history").join(session_id);
        if fh_dir.exists() {
            if try_remove_dir(&fh_dir).await {
                associated_deleted += 1;
            }
        }

        // 4. Todos (glob match: {session_id}-*.json)
        let todos_dir = claude_base.join("todos");
        if let Ok(mut entries) = tokio::fs::read_dir(&todos_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&format!("{}-", session_id)) && name.ends_with(".json") {
                    if try_remove_file(&entry.path()).await {
                        associated_deleted += 1;
                    }
                }
            }
        }

        // 5. Debug logs (glob match: *{session_id}*.txt)
        let debug_dir = claude_base.join("debug");
        if let Ok(mut entries) = tokio::fs::read_dir(&debug_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.contains(session_id) && name.ends_with(".txt") {
                    if try_remove_file(&entry.path()).await {
                        associated_deleted += 1;
                    }
                }
            }
        }

        // 6. security_warnings_state
        let sec_path = claude_base.join(format!(
            "security_warnings_state_{}.json",
            session_id
        ));
        if sec_path.exists() {
            if try_remove_file(&sec_path).await {
                associated_deleted += 1;
            }
        }

        // 7. session-env
        let env_dir = claude_base.join("session-env").join(session_id);
        if env_dir.exists() {
            if try_remove_dir(&env_dir).await {
                associated_deleted += 1;
            }
        }

        // 8. tasks
        let tasks_dir = claude_base.join("tasks").join(session_id);
        if tasks_dir.exists() {
            if try_remove_dir(&tasks_dir).await {
                associated_deleted += 1;
            }
        }

        // 9. Plans (glob match: *{session_id}*.md)
        let plans_dir = claude_base.join("plans");
        if let Ok(mut entries) = tokio::fs::read_dir(&plans_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.contains(session_id) && name.ends_with(".md") {
                    if try_remove_file(&entry.path()).await {
                        associated_deleted += 1;
                    }
                }
            }
        }

        // 10. Clean up sessions-index.json
        let index_path = project_dir.join("sessions-index.json");
        if index_path.exists() {
            if let Ok(content) = tokio::fs::read_to_string(&index_path).await {
                if let Ok(mut index) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(sessions_arr) =
                        index.get_mut("sessions").and_then(|v| v.as_array_mut())
                    {
                        let before = sessions_arr.len();
                        sessions_arr.retain(|s| {
                            s.get("sessionId")
                                .or_else(|| s.get("session_id"))
                                .and_then(|v| v.as_str())
                                .map(|id| id != session_id)
                                .unwrap_or(true)
                        });
                        if sessions_arr.len() < before {
                            if let Ok(updated) = serde_json::to_string_pretty(&index) {
                                if tokio::fs::write(&index_path, updated).await.is_ok() {
                                    info!(
                                        "Updated sessions-index.json for deleted session {}",
                                        session_id
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(DeleteFilesResult { associated_deleted })
    }
}
