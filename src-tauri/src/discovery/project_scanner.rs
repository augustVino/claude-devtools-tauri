//! ProjectScanner service - Scans ~/.claude/projects/ directory and lists all projects.
//!
//! Responsibilities:
//! - Read project directories from ~/.claude/projects/
//! - Decode directory names to original paths
//! - List session files for each project
//! - Return sorted list of projects by recent activity

use crate::types::domain::{Project, Session, SessionMetadataLevel};
use crate::utils::path_decoder;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// ProjectScanner scans the ~/.claude/projects/ directory for projects and sessions.
pub struct ProjectScanner {
    projects_dir: PathBuf,
    todos_dir: PathBuf,
}

impl ProjectScanner {
    /// Create a new ProjectScanner with default paths.
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let claude_base = home.join(".claude");
        Self {
            projects_dir: claude_base.join("projects"),
            todos_dir: claude_base.join("todos"),
        }
    }

    /// Create a new ProjectScanner with custom paths (for testing).
    pub fn with_paths(projects_dir: PathBuf, todos_dir: PathBuf) -> Self {
        Self { projects_dir, todos_dir }
    }

    /// Get the projects directory path.
    pub fn get_projects_dir(&self) -> &Path {
        &self.projects_dir
    }

    /// Get the todos directory path.
    pub fn get_todos_dir(&self) -> &Path {
        &self.todos_dir
    }

    /// Check if the projects directory exists.
    pub fn projects_dir_exists(&self) -> bool {
        self.projects_dir.exists()
    }

    /// Scan all projects and return them sorted by most recent activity.
    pub fn scan(&self) -> Vec<Project> {
        if !self.projects_dir.exists() {
            return Vec::new();
        }

        let entries = match fs::read_dir(&self.projects_dir) {
            Ok(entries) => entries,
            Err(_) => return Vec::new(),
        };

        let mut projects: Vec<Project> = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let encoded_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name,
                None => continue,
            };

            // Check if it's a valid encoded path
            if !path_decoder::is_valid_encoded_path(encoded_name) {
                continue;
            }

            if let Some(project) = self.scan_project(encoded_name) {
                projects.push(project);
            }
        }

        // Sort by most recent session (descending)
        projects.sort_by(|a, b| {
            b.most_recent_session.unwrap_or(0).cmp(&a.most_recent_session.unwrap_or(0))
        });

        projects
    }

    /// Scan a single project directory and return project metadata.
    fn scan_project(&self, encoded_name: &str) -> Option<Project> {
        let project_path = self.projects_dir.join(encoded_name);

        let entries = match fs::read_dir(&project_path) {
            Ok(entries) => entries,
            Err(_) => return None,
        };

        // Get session files (.jsonl at root level)
        let session_files: Vec<_> = entries
            .flatten()
            .filter(|entry| {
                entry.path().is_file() && entry.path().extension().map_or(false, |ext| ext == "jsonl")
            })
            .collect();

        if session_files.is_empty() {
            return None;
        }

        // Extract session IDs and compute timestamps
        let mut session_ids: Vec<String> = Vec::new();
        let mut most_recent_session: Option<u64> = None;
        let mut created_at = u64::MAX;

        for entry in &session_files {
            let file_name = entry.file_name();
            let file_name_str = file_name.to_string_lossy();

            let session_id = path_decoder::extract_session_id(&file_name_str);
            session_ids.push(session_id);

            if let Ok(metadata) = entry.metadata() {
                if let Ok(modified) = metadata.modified() {
                    let mtime = modified
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);

                    most_recent_session = Some(most_recent_session.map_or(mtime, |m| m.max(mtime)));
                }

                if let Ok(created) = metadata.created() {
                    let birthtime = created
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(u64::MAX);

                    created_at = created_at.min(birthtime);
                }
            }
        }

        let base_name = path_decoder::extract_project_name(encoded_name, None);
        let actual_path = self.resolve_project_path(encoded_name);

        Some(Project {
            id: encoded_name.to_string(),
            path: actual_path,
            name: base_name,
            sessions: session_ids,
            created_at: if created_at == u64::MAX { 0 } else { created_at / 1000 },
            most_recent_session: most_recent_session.map(|t| t / 1000),
        })
    }

    /// Get a specific project by ID.
    pub fn get_project(&self, project_id: &str) -> Option<Project> {
        let base_dir = path_decoder::extract_base_dir(project_id);
        self.scan_project(&base_dir)
    }

    /// List all sessions for a project.
    pub fn list_sessions(&self, project_id: &str) -> Vec<Session> {
        let base_dir = path_decoder::extract_base_dir(project_id);
        let project_path = self.projects_dir.join(&base_dir);

        if !project_path.exists() {
            return Vec::new();
        }

        let entries = match fs::read_dir(&project_path) {
            Ok(entries) => entries,
            Err(_) => return Vec::new(),
        };

        let mut sessions: Vec<Session> = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || path.extension().map_or(true, |ext| ext != "jsonl") {
                continue;
            }

            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let session_id = path_decoder::extract_session_id(file_name);

            // Get file metadata
            let metadata = entry.metadata().ok();
            let created_at = metadata
                .and_then(|m| m.created().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);

            let decoded_path = self.resolve_project_path(&base_dir);

            sessions.push(Session {
                id: session_id,
                project_id: project_id.to_string(),
                project_path: decoded_path,
                created_at: created_at / 1000,
                todo_data: self.load_todo_data(&file_name.trim_end_matches(".jsonl")),
                first_message: None,
                message_timestamp: None,
                has_subagents: false,
                message_count: 0,
                is_ongoing: None,
                git_branch: None,
                metadata_level: Some(SessionMetadataLevel::Light),
                context_consumption: None,
                compaction_count: None,
                phase_breakdown: None,
            });
        }

        // Sort by created date (most recent first)
        sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        sessions
    }

    /// Get the path to a session file.
    pub fn get_session_path(&self, project_id: &str, session_id: &str) -> PathBuf {
        let base_dir = path_decoder::extract_base_dir(project_id);
        self.projects_dir.join(&base_dir).join(format!("{}.jsonl", session_id))
    }

    /// Resolve the project path from encoded name.
    fn resolve_project_path(&self, encoded_name: &str) -> String {
        path_decoder::decode_path(encoded_name)
    }

    /// Load todo data for a session.
    fn load_todo_data(&self, session_id: &str) -> Option<serde_json::Value> {
        let todo_path = self.todos_dir.join(format!("{}.json", session_id));

        if !todo_path.exists() {
            return None;
        }

        fs::read_to_string(&todo_path)
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
    }

    /// List all session file paths for a project.
    pub fn list_session_files(&self, project_id: &str) -> Vec<String> {
        let base_dir = path_decoder::extract_base_dir(project_id);
        let project_path = self.projects_dir.join(&base_dir);

        if !project_path.exists() {
            return Vec::new();
        }

        let entries = match fs::read_dir(&project_path) {
            Ok(entries) => entries,
            Err(_) => return Vec::new(),
        };

        entries
            .flatten()
            .filter(|entry| {
                entry.path().is_file() && entry.path().extension().map_or(false, |ext| ext == "jsonl")
            })
            .map(|entry| entry.path().to_string_lossy().to_string())
            .collect()
    }
}

impl Default for ProjectScanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_env() -> (TempDir, ProjectScanner) {
        let temp_dir = TempDir::new().unwrap();
        let projects_dir = temp_dir.path().join("projects");
        let todos_dir = temp_dir.path().join("todos");
        fs::create_dir_all(&projects_dir).unwrap();
        fs::create_dir_all(&todos_dir).unwrap();

        let scanner = ProjectScanner::with_paths(projects_dir, todos_dir);
        (temp_dir, scanner)
    }

    #[test]
    fn test_projects_dir_exists() {
        let (temp_dir, scanner) = setup_test_env();
        assert!(scanner.projects_dir_exists());
    }

    #[test]
    fn test_scan_empty() {
        let (_temp_dir, scanner) = setup_test_env();
        let projects = scanner.scan();
        assert!(projects.is_empty());
    }

    #[test]
    fn test_scan_with_project() {
        let (temp_dir, scanner) = setup_test_env();

        // Create a project directory with encoded path
        let encoded_path = "-Users-test-myproject";
        let project_dir = temp_dir.path().join("projects").join(encoded_path);
        fs::create_dir_all(&project_dir).unwrap();

        // Create a session file
        let session_path = project_dir.join("test-session-id.jsonl");
        fs::write(&session_path, r#"{"type":"user","message":"hello"}"#).unwrap();

        let projects = scanner.scan();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, encoded_path);
        assert!(projects[0].sessions.contains(&"test-session-id".to_string()));
    }

    #[test]
    fn test_get_session_path() {
        let (_temp_dir, scanner) = setup_test_env();

        let path = scanner.get_session_path("-Users-test-project", "session-123");
        assert!(path.to_string_lossy().ends_with("session-123.jsonl"));
    }

    #[test]
    fn test_list_sessions() {
        let (temp_dir, scanner) = setup_test_env();

        // Create a project directory
        let encoded_path = "-Users-test-myproject";
        let project_dir = temp_dir.path().join("projects").join(encoded_path);
        fs::create_dir_all(&project_dir).unwrap();

        // Create session files
        fs::write(project_dir.join("session-1.jsonl"), r#"{"type":"user"}"#).unwrap();
        fs::write(project_dir.join("session-2.jsonl"), r#"{"type":"user"}"#).unwrap();

        let sessions = scanner.list_sessions(encoded_path);
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn test_list_session_files() {
        let (temp_dir, scanner) = setup_test_env();

        // Create a project directory
        let encoded_path = "-Users-test-myproject";
        let project_dir = temp_dir.path().join("projects").join(encoded_path);
        fs::create_dir_all(&project_dir).unwrap();

        // Create session files
        fs::write(project_dir.join("session-1.jsonl"), "").unwrap();
        fs::write(project_dir.join("session-2.jsonl"), "").unwrap();

        let files = scanner.list_session_files(encoded_path);
        assert_eq!(files.len(), 2);
    }
}