//! ProjectScanner service - Scans ~/.claude/projects/ directory and lists all projects.
//!
//! Responsibilities:
//! - Read project directories from ~/.claude/projects/
//! - Decode directory names to original paths
//! - List session files for each project
//! - Return sorted list of projects by recent activity

use crate::infrastructure::fs_provider::{FsProvider, LocalFsProvider};
use crate::types::domain::{Project, Session, SessionMetadataLevel};
use crate::utils::content_sanitizer::{
    extract_command_display, sanitize_display_content, is_command_output_content, is_command_content,
};
use crate::utils::path_decoder;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Light-weight session preview extracted from the first few KB of a JSONL file.
struct SessionPreview {
    first_message: Option<String>,
    first_timestamp: Option<String>,
    has_task_calls: bool,
    message_count: u32,
    is_ongoing: Option<bool>,
    git_branch: Option<String>,
    /// Stored command display as fallback title (e.g., "/model sonnet").
    command_fallback: Option<String>,
    /// Current working directory from the first entry that has a `cwd` field.
    cwd: Option<String>,
}

impl Default for SessionPreview {
    fn default() -> Self {
        Self {
            first_message: None,
            first_timestamp: None,
            has_task_calls: false,
            command_fallback: None,
            message_count: 0,
            is_ongoing: None,
            git_branch: None,
            cwd: None,
        }
    }
}

/// ProjectScanner scans the ~/.claude/projects/ directory for projects and sessions.
pub struct ProjectScanner {
    projects_dir: PathBuf,
    todos_dir: PathBuf,
    fs_provider: Arc<dyn FsProvider>,
}

impl ProjectScanner {
    /// Create a new ProjectScanner with default paths.
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let claude_base = home.join(".claude");
        Self {
            projects_dir: claude_base.join("projects"),
            todos_dir: claude_base.join("todos"),
            fs_provider: Arc::new(LocalFsProvider::new()),
        }
    }

    /// Create a new ProjectScanner with custom paths (for testing).
    pub fn with_paths(
        projects_dir: PathBuf,
        todos_dir: PathBuf,
        fs_provider: Arc<dyn FsProvider>,
    ) -> Self {
        Self {
            projects_dir,
            todos_dir,
            fs_provider,
        }
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
        self.fs_provider
            .exists(&self.projects_dir)
            .unwrap_or(false)
    }

    /// Scan all projects and return them sorted by most recent activity.
    pub fn scan(&self) -> Vec<Project> {
        if !self
            .fs_provider
            .exists(&self.projects_dir)
            .unwrap_or(false)
        {
            return Vec::new();
        }

        let entries = match self.fs_provider.read_dir(&self.projects_dir) {
            Ok(entries) => entries,
            Err(_) => return Vec::new(),
        };

        let mut projects: Vec<Project> = Vec::new();

        for dirent in &entries {
            if !dirent.is_directory {
                continue;
            }

            let encoded_name = &dirent.name;

            // Check if it's a valid encoded path
            if !path_decoder::is_valid_encoded_path(encoded_name) {
                continue;
            }

            projects.extend(self.scan_project(encoded_name));
        }

        // Sort by most recent session (descending)
        projects.sort_by(|a, b| {
            b.most_recent_session.unwrap_or(0).cmp(&a.most_recent_session.unwrap_or(0))
        });

        projects
    }

    /// Scan a single project directory and return project metadata.
    pub fn scan_project(&self, encoded_name: &str) -> Vec<Project> {
        let project_path = self.projects_dir.join(encoded_name);

        let entries = match self.fs_provider.read_dir(&project_path) {
            Ok(entries) => entries,
            Err(_) => return vec![],
        };

        // Get session files (.jsonl at root level)
        let session_files: Vec<_> = entries
            .iter()
            .filter(|dirent| {
                dirent.is_file
                    && Path::new(&dirent.name)
                        .extension()
                        .map_or(false, |ext| ext == "jsonl")
            })
            .collect();

        if session_files.is_empty() {
            return vec![];
        }

        // Extract session IDs, timestamps, and cwd from session files
        let mut session_ids: Vec<String> = Vec::new();
        let mut most_recent_session: Option<u64> = None;
        let mut created_at = u64::MAX;
        let mut first_cwd: Option<String> = None;

        for session_file in &session_files {
            let session_id = path_decoder::extract_session_id(&session_file.name);
            session_ids.push(session_id);

            if let Some(mtime) = session_file.mtime_ms {
                most_recent_session =
                    Some(most_recent_session.map_or(mtime, |m| m.max(mtime)));
            }

            if let Some(birthtime) = session_file.birthtime_ms {
                created_at = created_at.min(birthtime);
            }

            // Extract cwd from the first session file that has one
            if first_cwd.is_none() {
                let entry_path = project_path.join(&session_file.name);
                let preview = self.extract_session_preview(&entry_path, session_file.mtime_ms);
                if let Some(cwd) = preview.cwd {
                    first_cwd = Some(cwd);
                }
            }
        }

        let base_name =
            path_decoder::extract_project_name(encoded_name, first_cwd.as_deref());
        let actual_path = first_cwd.unwrap_or_else(|| self.resolve_project_path(encoded_name));

        vec![Project {
            id: encoded_name.to_string(),
            path: actual_path,
            name: base_name,
            sessions: session_ids,
            created_at: if created_at == u64::MAX { 0 } else { created_at },
            most_recent_session,
        }]
    }

    /// Get a specific project by ID.
    pub fn get_project(&self, project_id: &str) -> Option<Project> {
        let base_dir = path_decoder::extract_base_dir(project_id);
        self.scan_project(&base_dir).into_iter().find(|p| p.id == project_id)
    }

    /// List all sessions for a project.
    pub fn list_sessions(&self, project_id: &str) -> Vec<Session> {
        let base_dir = path_decoder::extract_base_dir(project_id);
        let project_path = self.projects_dir.join(&base_dir);

        if !self.fs_provider.exists(&project_path).unwrap_or(false) {
            return Vec::new();
        }

        let entries = match self.fs_provider.read_dir(&project_path) {
            Ok(entries) => entries,
            Err(_) => return Vec::new(),
        };

        // Step 1: Collect file entries with metadata (lightweight stat calls)
        struct FileInfo {
            name: String,
            session_id: String,
            mtime_ms: u64,
            birthtime_ms: u64,
        }

        let mut file_infos: Vec<FileInfo> = Vec::new();

        for dirent in &entries {
            if !dirent.is_file {
                continue;
            }
            if !Path::new(&dirent.name)
                .extension()
                .map_or(false, |ext| ext == "jsonl")
            {
                continue;
            }

            let session_id = path_decoder::extract_session_id(&dirent.name);

            let mtime_ms = dirent.mtime_ms.unwrap_or(0);
            let birthtime_ms = dirent.birthtime_ms.unwrap_or(0);

            file_infos.push(FileInfo {
                name: dirent.name.clone(),
                session_id,
                mtime_ms,
                birthtime_ms,
            });
        }

        // Step 2: Sort by file modification time (most recent first), matching Electron's mtimeMs sort.
        // Tie-breaker: session ID alphabetical ascending (stable ordering).
        file_infos.sort_by(|a, b| {
            if b.mtime_ms != a.mtime_ms {
                return b.mtime_ms.cmp(&a.mtime_ms);
            }
            a.session_id.cmp(&b.session_id)
        });

        // Step 3: Build Session objects from sorted file entries
        let mut sessions: Vec<Session> = Vec::new();

        for info in &file_infos {
            let entry_path = project_path.join(&info.name);

            // Skip noise-only sessions (local filesystem only)
            if self.fs_provider.provider_type() != "ssh" {
                if !crate::discovery::session_content_filter::has_non_noise_messages(&entry_path, self.fs_provider.as_ref()) {
                    continue;
                }
            }

            let preview = self.extract_session_preview(&entry_path, Some(info.mtime_ms));

            // Skip sessions that couldn't be read (file may have been deleted or is empty)
            if preview.message_count == 0 && preview.first_message.is_none() {
                log::debug!("Skipping empty or unreadable session file: {}", entry_path.display());
                continue;
            }

            let decoded_path = preview
                .cwd
                .unwrap_or_else(|| self.resolve_project_path(&base_dir));

            // createdAt: use first message timestamp from JSONL, fallback to file birth time.
            // This matches Electron's buildSessionMetadata() behavior for date grouping.
            let created_at = preview
                .first_timestamp
                .as_ref()
                .and_then(|ts| {
                    chrono::DateTime::parse_from_rfc3339(ts)
                        .or_else(|_| chrono::DateTime::parse_from_rfc2822(ts))
                        .ok()
                        .and_then(|dt| dt.timestamp_millis().try_into().ok())
                })
                .unwrap_or(info.birthtime_ms);

            let file_name = info.name.as_str();

            sessions.push(Session {
                id: info.session_id.clone(),
                project_id: project_id.to_string(),
                project_path: decoded_path,
                created_at,
                todo_data: self.load_todo_data(file_name.trim_end_matches(".jsonl")),
                first_message: preview.first_message,
                message_timestamp: preview.first_timestamp,
                has_subagents: preview.has_task_calls,
                message_count: preview.message_count,
                is_ongoing: preview.is_ongoing,
                git_branch: preview.git_branch,
                metadata_level: Some(SessionMetadataLevel::Light),
                context_consumption: None,
                compaction_count: None,
                phase_breakdown: None,
            });
        }

        sessions
    }

    /// Read the first 200 lines of a JSONL file to extract preview metadata.
    fn extract_session_preview(&self, path: &Path, mtime_ms: Option<u64>) -> SessionPreview {
        let content = match self.fs_provider.read_file_head(path, 200) {
            Ok(content) => content,
            Err(_) => return SessionPreview::default(),
        };

        let mut preview = SessionPreview::default();
        let mut found_first_user = false;
        let mut lines_read = 0u32;
        let mut awaiting_ai_group = false;
        const MAX_LINES: u32 = 200;

        for line in content.lines() {
            if lines_read >= MAX_LINES {
                break;
            }
            lines_read += 1;

            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let Ok(json) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };

            let msg_type = json.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let is_sidechain = json.get("isSidechain").and_then(|v| v.as_bool()).unwrap_or(false);
            let model = json.get("model").and_then(|v| v.as_str());

            // Extract cwd from any entry that has it (first wins)
            if preview.cwd.is_none() {
                if let Some(cwd) = json.get("cwd").and_then(|v| v.as_str()) {
                    if !cwd.is_empty() {
                        preview.cwd = Some(normalize_drive_letter(
                            translate_wsl_mount_path(cwd),
                        ));
                    }
                }
            }

            // Extract git_branch from any entry that has it (first wins)
            if preview.git_branch.is_none() {
                if let Some(branch) = json.get("gitBranch").and_then(|v| v.as_str()) {
                    preview.git_branch = Some(branch.to_string());
                }
            }

            // Paired message_count: count user then matching assistant
            if crate::parsing::message_classifier::is_user_chunk_message(msg_type, is_sidechain) {
                preview.message_count += 1;
                awaiting_ai_group = true;
            } else if awaiting_ai_group && msg_type == "assistant"
                && model != Some("<synthetic>") && !is_sidechain {
                preview.message_count += 1;
                awaiting_ai_group = false;
            }

            if !found_first_user && msg_type == "user" {
                // Extract first message text — handle both string and array content.
                // Note: Electron does NOT check isMeta for title extraction.
                // Slash commands are meta messages (isMeta: true) — we must still
                // detect them as command fallback titles.
                let msg_content = json.pointer("/message/content");
                let text = if let Some(s) = msg_content.and_then(|v| v.as_str()) {
                    Some(s.to_string())
                } else if let Some(arr) = msg_content.and_then(|v| v.as_array()) {
                    let parts: Vec<String> = arr
                        .iter()
                        .filter_map(|block| {
                            if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                                block.get("text").and_then(|t| t.as_str()).map(|s| s.trim().to_string())
                            } else {
                                None
                            }
                        })
                        .collect();
                    if parts.is_empty() {
                        None
                    } else {
                        Some(parts.join(" "))
                    }
                } else {
                    None
                };

                if let Some(text) = text {
                    let trimmed = text.trim();

                    // Skip command output and interruptions
                    if is_command_output_content(trimmed)
                        || trimmed.starts_with("[Request interrupted by user")
                    {
                        // Still capture metadata even when skipping
                        if preview.first_timestamp.is_none() {
                            if let Some(ts) =
                                json.get("timestamp").and_then(|v| v.as_str())
                            {
                                preview.first_timestamp = Some(ts.to_string());
                            }
                        }
                        continue;
                    }

                    // Store command-name as fallback, keep looking for real text.
                    // Match Electron's `content.startsWith('<command-name>')` check exactly:
                    // content starting with <command-message> (without <command-name>) is NOT
                    // treated as a command here — it falls through to sanitization which
                    // extracts the display via sanitize_display_content().
                    if trimmed.starts_with("<command-name>") {
                        if preview.command_fallback.is_none() {
                            preview.command_fallback =
                                extract_command_display(trimmed);
                        }
                    } else {
                        // Real user text found — sanitize and truncate to 500 chars.
                        // Note: Electron does NOT check isMeta here; meta messages with
                        // text content are used as titles (e.g. skill invocation messages).
                        let sanitized = sanitize_display_content(trimmed);
                        if !sanitized.is_empty() {
                            preview.first_message =
                                Some(sanitized.chars().take(500).collect());
                            found_first_user = true;
                        }
                    }
                }

                if let Some(ts) = json.get("timestamp").and_then(|v| v.as_str()) {
                    preview.first_timestamp = Some(ts.to_string());
                }
            }

            if !preview.has_task_calls {
                if let Some(content) = json.pointer("/message/content") {
                    if let Some(arr) = content.as_array() {
                        for block in arr {
                            if block.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                                && block.get("name").and_then(|t| t.as_str()) == Some("Task")
                            {
                                preview.has_task_calls = true;
                                break;
                            }
                        }
                    }
                }
            }
        }

        // Fall back to command display if no real user text was found
        if preview.first_message.is_none() {
            preview.first_message = preview.command_fallback.take();
        }

        // Stale session threshold: if is_ongoing but file hasn't been modified
        // in 5+ minutes, mark as not ongoing.
        const STALE_SESSION_THRESHOLD_MS: u64 = 5 * 60 * 1000;
        if preview.is_ongoing.unwrap_or(false) {
            if let Some(file_mtime) = mtime_ms {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                if now.saturating_sub(file_mtime) >= STALE_SESSION_THRESHOLD_MS {
                    preview.is_ongoing = Some(false);
                }
            }
        }

        preview
    }

    /// Get a single session's metadata by its ID within a project.
    /// More efficient than list_sessions + find when you only need one session.
    pub fn get_session_by_id(&self, project_id: &str, session_id: &str) -> Option<Session> {
        let base_dir = path_decoder::extract_base_dir(project_id);
        let project_path = self.projects_dir.join(&base_dir);
        let session_file_name = format!("{}.jsonl", session_id);
        let session_path = project_path.join(&session_file_name);

        if !self.fs_provider.exists(&session_path).unwrap_or(false) {
            return None;
        }

        // Get file stat for timestamps
        let stat = self.fs_provider.stat(&session_path).ok()?;
        let mtime_ms = stat.mtime_ms;
        let birthtime_ms = stat.birthtime_ms;

        // Skip noise-only sessions (local filesystem only)
        if self.fs_provider.provider_type() != "ssh" {
            if !crate::discovery::session_content_filter::has_non_noise_messages(&session_path, self.fs_provider.as_ref()) {
                return None;
            }
        }

        // Extract preview (first message, message count, etc.)
        let preview = self.extract_session_preview(&session_path, Some(mtime_ms));

        // Skip sessions that couldn't be read
        if preview.message_count == 0 && preview.first_message.is_none() {
            return None;
        }

        let decoded_path = preview
            .cwd
            .unwrap_or_else(|| self.resolve_project_path(&base_dir));

        let created_at = preview
            .first_timestamp
            .as_ref()
            .and_then(|ts| {
                chrono::DateTime::parse_from_rfc3339(ts)
                    .or_else(|_| chrono::DateTime::parse_from_rfc2822(ts))
                    .ok()
                    .and_then(|dt| dt.timestamp_millis().try_into().ok())
            })
            .unwrap_or(birthtime_ms);

        Some(Session {
            id: session_id.to_string(),
            project_id: project_id.to_string(),
            project_path: decoded_path,
            created_at,
            todo_data: self.load_todo_data(session_id),
            first_message: preview.first_message,
            message_timestamp: preview.first_timestamp,
            has_subagents: preview.has_task_calls,
            message_count: preview.message_count,
            is_ongoing: preview.is_ongoing,
            git_branch: preview.git_branch,
            metadata_level: Some(SessionMetadataLevel::Light),
            context_consumption: None,
            compaction_count: None,
            phase_breakdown: None,
        })
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

        if !self.fs_provider.exists(&todo_path).unwrap_or(false) {
            return None;
        }

        self.fs_provider
            .read_file(&todo_path)
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
    }

    /// List all session file paths for a project.
    pub fn list_session_files(&self, project_id: &str) -> Vec<String> {
        let base_dir = path_decoder::extract_base_dir(project_id);
        let project_path = self.projects_dir.join(&base_dir);

        if !self.fs_provider.exists(&project_path).unwrap_or(false) {
            return Vec::new();
        }

        let entries = match self.fs_provider.read_dir(&project_path) {
            Ok(entries) => entries,
            Err(_) => return Vec::new(),
        };

        entries
            .iter()
            .filter(|dirent| {
                dirent.is_file
                    && Path::new(&dirent.name)
                        .extension()
                        .map_or(false, |ext| ext == "jsonl")
            })
            .map(|dirent| project_path.join(&dirent.name).to_string_lossy().to_string())
            .collect()
    }
}

/// Normalize Windows drive letter to uppercase for consistent path comparison.
/// CLI uses uppercase (C:\...) while VS Code extension uses lowercase (c:\...).
fn normalize_drive_letter(p: String) -> String {
    let bytes = p.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' {
        let mut chars: Vec<char> = p.chars().collect();
        chars[0] = chars[0].to_ascii_uppercase();
        chars.into_iter().collect()
    } else {
        p
    }
}

/// Translate WSL mount paths (e.g., /mnt/c/...) to Windows drive paths (C:/...).
fn translate_wsl_mount_path(p: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        if let Some(rest) = p.strip_prefix("/mnt/") {
            if let Some(drive) = rest.chars().next().filter(|c| c.is_ascii_alphabetic()) {
                let rem = &rest[drive.len_utf8()..];
                let sep = if rem.is_empty() || rem.starts_with('/') { "" } else { "/" };
                return format!("{}:{}{}", drive.to_ascii_uppercase(), sep, rem);
            }
        }
    }
    p.to_string()
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

        let scanner = ProjectScanner::with_paths(
            projects_dir,
            todos_dir,
            Arc::new(LocalFsProvider::new()),
        );
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

        // Create session files (with assistant entries so noise filter passes)
        fs::write(project_dir.join("session-1.jsonl"), r#"{"type":"user","isSidechain":false,"message":{"role":"user","content":"hello"}}
{"type":"assistant","message":{"role":"assistant","content":"hi"}}"#).unwrap();
        fs::write(project_dir.join("session-2.jsonl"), r#"{"type":"user","isSidechain":false,"message":{"role":"user","content":"world"}}
{"type":"assistant","message":{"role":"assistant","content":"hi"}}"#).unwrap();

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

    #[test]
    fn test_scan_project_uses_cwd_for_name_and_path() {
        let (temp_dir, scanner) = setup_test_env();

        // Encoded name that would lossy-decode to "server" instead of "happy-server"
        // /Users/test/happy-server → -Users-test-happy-server
        // decode_path: -Users-test-happy-server → /Users/test/happy/server (WRONG, lossy)
        let encoded_path = "-Users-test-happy-server";
        let project_dir = temp_dir.path().join("projects").join(encoded_path);
        fs::create_dir_all(&project_dir).unwrap();

        // Session file with correct cwd in JSONL
        let jsonl = r#"{"type":"user","cwd":"/Users/test/happy-server","message":{"role":"user","content":"hello"}}"#;
        let session_path = project_dir.join("test-session.jsonl");
        fs::write(&session_path, jsonl).unwrap();

        let projects = scanner.scan();
        assert_eq!(projects.len(), 1);

        let project = &projects[0];
        // Name should come from cwd, not from lossy decode
        assert_eq!(project.name, "happy-server", "name should be 'happy-server' from cwd, not 'server' from lossy decode");
        // Path should come from cwd
        assert_eq!(project.path, "/Users/test/happy-server", "path should use cwd");
    }

    #[test]
    fn test_scan_project_falls_back_to_decode_without_cwd() {
        let (temp_dir, scanner) = setup_test_env();

        let encoded_path = "-Users-test-myproject";
        let project_dir = temp_dir.path().join("projects").join(encoded_path);
        fs::create_dir_all(&project_dir).unwrap();

        // Session file WITHOUT cwd field
        let jsonl = r#"{"type":"user","message":{"role":"user","content":"hello"}}"#;
        fs::write(project_dir.join("test-session.jsonl"), jsonl).unwrap();

        let projects = scanner.scan();
        assert_eq!(projects.len(), 1);

        let project = &projects[0];
        // Should fall back to lossy decode
        assert_eq!(project.name, "myproject");
    }

    #[test]
    fn test_list_sessions_uses_cwd_for_project_path() {
        let (temp_dir, scanner) = setup_test_env();

        let encoded_path = "-Users-test-happy-server";
        let project_dir = temp_dir.path().join("projects").join(encoded_path);
        fs::create_dir_all(&project_dir).unwrap();

        let jsonl = r#"{"type":"user","isSidechain":false,"cwd":"/Users/test/happy-server","message":{"role":"user","content":"hello"}}
{"type":"assistant","message":{"role":"assistant","content":"response"}}"#;
        fs::write(project_dir.join("session-1.jsonl"), jsonl).unwrap();

        let sessions = scanner.list_sessions(encoded_path);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].project_path, "/Users/test/happy-server");
    }

    #[test]
    fn test_normalize_drive_letter() {
        assert_eq!(normalize_drive_letter("c:/Users/test".to_string()), "C:/Users/test");
        assert_eq!(normalize_drive_letter("C:/Users/test".to_string()), "C:/Users/test");
        assert_eq!(normalize_drive_letter("/Users/test".to_string()), "/Users/test");
    }

    // --- Task 1: Bug reproduction test ---
    #[test]
    fn test_extract_session_preview_first_message_beyond_8kb() {
        let (temp_dir, scanner) = setup_test_env();

        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("beyond-8kb.jsonl");

        let mut content = String::new();
        for i in 0..50 {
            content.push_str(&format!(
                r#"{{"type":"system","message":{{"role":"system","content":"System padding line {} with extra text to ensure it exceeds eight kilobytes total across all lines combined in the file"}}}}
"#,
                i
            ));
        }
        content.push_str(r#"{"type":"user","isMeta":false,"message":{"role":"user","content":"My real session title"}}"#);

        fs::write(&session_path, &content).unwrap();

        let preview = scanner.extract_session_preview(&session_path, None);
        assert!(
            preview.first_message.is_some(),
            "first_message should be found when user message is beyond 8KB"
        );
        assert_eq!(preview.first_message.unwrap(), "My real session title");
    }

    // --- Task 3: first_message tests ---
    #[test]
    fn test_extract_session_preview_first_message_simple() {
        let (temp_dir, scanner) = setup_test_env();
        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("simple.jsonl");

        let jsonl = r#"{"type":"user","isMeta":false,"message":{"role":"user","content":"Hello world"}}"#;
        fs::write(&session_path, jsonl).unwrap();

        let preview = scanner.extract_session_preview(&session_path, None);
        assert_eq!(preview.first_message.as_deref(), Some("Hello world"));
    }

    #[test]
    fn test_extract_session_preview_skips_meta_user() {
        let (temp_dir, scanner) = setup_test_env();
        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("meta.jsonl");

        // Electron does NOT check isMeta for title extraction — meta user messages
        // with displayable text are used as titles (matching Electron behavior).
        let jsonl = r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"internal meta"}}
{"type":"user","isMeta":false,"message":{"role":"user","content":"real user text"}}"#;
        fs::write(&session_path, jsonl).unwrap();

        let preview = scanner.extract_session_preview(&session_path, None);
        assert_eq!(preview.first_message.as_deref(), Some("internal meta"));
    }

    #[test]
    fn test_extract_session_preview_command_fallback() {
        let (temp_dir, scanner) = setup_test_env();
        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("cmd.jsonl");

        let jsonl = r#"{"type":"user","isMeta":false,"message":{"role":"user","content":"<command-name>/compact</command-name><command-message>Compact context</command-message>"}}"#;
        fs::write(&session_path, jsonl).unwrap();

        let preview = scanner.extract_session_preview(&session_path, None);
        assert_eq!(preview.first_message.as_deref(), Some("/compact"));
    }

    #[test]
    fn test_extract_session_preview_skips_command_output() {
        let (temp_dir, scanner) = setup_test_env();
        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("cmdout.jsonl");

        let jsonl = r#"{"type":"user","isMeta":false,"message":{"role":"user","content":"<local-command-stdout>output</local-command-stdout>"}}
{"type":"user","isMeta":false,"message":{"role":"user","content":"real message"}}"#;
        fs::write(&session_path, jsonl).unwrap();

        let preview = scanner.extract_session_preview(&session_path, None);
        assert_eq!(preview.first_message.as_deref(), Some("real message"));
    }

    #[test]
    fn test_extract_session_preview_sanitizes_content() {
        let (temp_dir, scanner) = setup_test_env();
        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("sanitize.jsonl");

        let jsonl = r#"{"type":"user","isMeta":false,"message":{"role":"user","content":"<system-reminder>rules</system-reminder>Please fix the bug"}}"#;
        fs::write(&session_path, jsonl).unwrap();

        let preview = scanner.extract_session_preview(&session_path, None);
        assert_eq!(preview.first_message.as_deref(), Some("Please fix the bug"));
    }

    #[test]
    fn test_extract_session_preview_truncates_to_500() {
        let (temp_dir, scanner) = setup_test_env();
        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("truncate.jsonl");

        let long_text = "a".repeat(600);
        let jsonl = format!(
            r#"{{"type":"user","isMeta":false,"message":{{"role":"user","content":"{}"}}}}"#,
            long_text
        );
        fs::write(&session_path, &jsonl).unwrap();

        let preview = scanner.extract_session_preview(&session_path, None);
        assert_eq!(preview.first_message.unwrap().chars().count(), 500);
    }

    // --- Task 3: metadata field tests ---
    #[test]
    fn test_extract_session_preview_extracts_cwd() {
        let (temp_dir, scanner) = setup_test_env();
        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("cwd.jsonl");

        let jsonl = r#"{"type":"user","isMeta":false,"cwd":"/Users/test/my-project","message":{"role":"user","content":"hello"}}"#;
        fs::write(&session_path, jsonl).unwrap();

        let preview = scanner.extract_session_preview(&session_path, None);
        assert_eq!(preview.cwd.as_deref(), Some("/Users/test/my-project"));
    }

    #[test]
    fn test_extract_session_preview_extracts_git_branch() {
        let (temp_dir, scanner) = setup_test_env();
        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("git.jsonl");

        let jsonl = r#"{"type":"user","isMeta":false,"gitBranch":"feature/test","message":{"role":"user","content":"hello"}}"#;
        fs::write(&session_path, jsonl).unwrap();

        let preview = scanner.extract_session_preview(&session_path, None);
        assert_eq!(preview.git_branch.as_deref(), Some("feature/test"));
    }

    #[test]
    fn test_extract_session_preview_message_count() {
        let (temp_dir, scanner) = setup_test_env();
        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("count.jsonl");

        let jsonl = r#"{"type":"user","message":{"role":"user","content":"hello"}}
{"type":"assistant","message":{"role":"assistant","content":"hi"}}
{"type":"user","message":{"role":"user","content":"world"}}
{"type":"system","message":{"role":"system","content":"sys"}}"#;
        fs::write(&session_path, jsonl).unwrap();

        let preview = scanner.extract_session_preview(&session_path, None);
        // Paired counting: user=1, assistant=1, user=1 (system not counted)
        assert_eq!(preview.message_count, 3);
    }

    #[test]
    fn test_extract_session_preview_has_task_calls() {
        let (temp_dir, scanner) = setup_test_env();
        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("task.jsonl");

        let jsonl = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"Task","input":{"prompt":"do something"}}]}}"#;
        fs::write(&session_path, jsonl).unwrap();

        let preview = scanner.extract_session_preview(&session_path, None);
        assert!(preview.has_task_calls);
    }

    #[test]
    fn test_extract_session_preview_first_timestamp() {
        let (temp_dir, scanner) = setup_test_env();
        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("ts.jsonl");

        let jsonl = r#"{"type":"user","isMeta":false,"timestamp":"2026-03-29T10:00:00Z","message":{"role":"user","content":"hello"}}"#;
        fs::write(&session_path, jsonl).unwrap();

        let preview = scanner.extract_session_preview(&session_path, None);
        assert_eq!(preview.first_timestamp.as_deref(), Some("2026-03-29T10:00:00Z"));
    }

    // --- Task 3: edge case tests ---
    #[test]
    fn test_extract_session_preview_empty_file() {
        let (temp_dir, scanner) = setup_test_env();
        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("empty.jsonl");

        fs::write(&session_path, "").unwrap();

        let preview = scanner.extract_session_preview(&session_path, None);
        assert!(preview.first_message.is_none());
        assert_eq!(preview.message_count, 0);
        assert!(!preview.has_task_calls);
    }

    #[test]
    fn test_extract_session_preview_nonexistent_file() {
        let (temp_dir, scanner) = setup_test_env();
        let path = temp_dir.path().join("nonexistent.jsonl");

        let preview = scanner.extract_session_preview(&path, None);
        assert!(preview.first_message.is_none());
        assert_eq!(preview.message_count, 0);
    }

    #[test]
    fn test_extract_session_preview_stops_at_200_lines() {
        let (temp_dir, scanner) = setup_test_env();
        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("200lines.jsonl");

        let mut content = String::new();
        for i in 0..199 {
            content.push_str(&format!(
                r#"{{"type":"system","message":{{"role":"system","content":"padding {}"}}}}
"#,
                i
            ));
        }
        content.push_str(r#"{"type":"user","isMeta":false,"message":{"role":"user","content":"found at line 200"}}"#);
        fs::write(&session_path, &content).unwrap();

        let preview = scanner.extract_session_preview(&session_path, None);
        assert_eq!(preview.first_message.as_deref(), Some("found at line 200"));
    }

    #[test]
    fn test_extract_session_preview_beyond_200_lines() {
        let (temp_dir, scanner) = setup_test_env();
        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();
        let session_path = project_dir.join("201lines.jsonl");

        let mut content = String::new();
        for i in 0..200 {
            content.push_str(&format!(
                r#"{{"type":"system","message":{{"role":"system","content":"padding {}"}}}}
"#,
                i
            ));
        }
        content.push_str(r#"{"type":"user","isMeta":false,"message":{"role":"user","content":"beyond 200 lines"}}"#);
        fs::write(&session_path, &content).unwrap();

        let preview = scanner.extract_session_preview(&session_path, None);
        assert!(preview.first_message.is_none(), "user message beyond 200 lines should not be found");
    }
}