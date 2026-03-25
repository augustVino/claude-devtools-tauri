//! ClaudeMdReader service - Reads CLAUDE.md files and calculates token counts.
//!
//! Responsibilities:
//! - Read CLAUDE.md files from various locations
//! - Calculate character counts and estimate token counts
//! - Handle file not found gracefully
//! - Support tilde (~) expansion to home directory

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Information about a CLAUDE.md file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClaudeMdFileInfo {
    pub path: String,
    pub exists: bool,
    #[serde(rename = "charCount")]
    pub char_count: usize,
    #[serde(rename = "estimatedTokens")]
    pub estimated_tokens: usize,
}

/// Result of reading all CLAUDE.md files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClaudeMdReadResult {
    pub files: HashMap<String, ClaudeMdFileInfo>,
}

/// ClaudeMdReader reads CLAUDE.md files and calculates token counts.
pub struct ClaudeMdReader {
    claude_base_path: PathBuf,
    home_dir: PathBuf,
}

impl ClaudeMdReader {
    /// Create a new ClaudeMdReader instance.
    pub fn new() -> Self {
        let home_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let claude_base_path = home_dir.join(".claude");
        Self { claude_base_path, home_dir }
    }

    /// Create with custom paths (for testing).
    pub fn with_paths(claude_base_path: PathBuf, home_dir: PathBuf) -> Self {
        Self { claude_base_path, home_dir }
    }

    /// Expand tilde (~) in a path.
    fn expand_tilde(&self, path: &str) -> PathBuf {
        if path.starts_with('~') {
            let remainder = path[1..].trim_start_matches('/');
            self.home_dir.join(remainder)
        } else {
            PathBuf::from(path)
        }
    }

    /// Estimate token count from content (rough: char_count / 4).
    fn estimate_tokens(content: &str) -> usize {
        content.len() / 4
    }

    /// Read a single CLAUDE.md file and return its info.
    pub fn read_claude_md_file(&self, file_path: &str) -> ClaudeMdFileInfo {
        let expanded_path = self.expand_tilde(file_path);

        match fs::read_to_string(&expanded_path) {
            Ok(content) => {
                let char_count = content.len();
                let estimated_tokens = Self::estimate_tokens(&content);
                ClaudeMdFileInfo {
                    path: expanded_path.to_string_lossy().to_string(),
                    exists: true,
                    char_count,
                    estimated_tokens,
                }
            }
            Err(_) => ClaudeMdFileInfo {
                path: expanded_path.to_string_lossy().to_string(),
                exists: false,
                char_count: 0,
                estimated_tokens: 0,
            },
        }
    }

    /// Read all .md files in a directory and return combined info.
    pub fn read_directory_md_files(&self, dir_path: &str) -> ClaudeMdFileInfo {
        let expanded_path = self.expand_tilde(dir_path);

        if !expanded_path.exists() || !expanded_path.is_dir() {
            return ClaudeMdFileInfo {
                path: expanded_path.to_string_lossy().to_string(),
                exists: false,
                char_count: 0,
                estimated_tokens: 0,
            };
        }

        let md_files = self.collect_md_files(&expanded_path);

        if md_files.is_empty() {
            return ClaudeMdFileInfo {
                path: expanded_path.to_string_lossy().to_string(),
                exists: false,
                char_count: 0,
                estimated_tokens: 0,
            };
        }

        let mut total_char_count = 0;
        let mut all_content = Vec::new();

        for file_path in &md_files {
            if let Ok(content) = fs::read_to_string(file_path) {
                total_char_count += content.len();
                all_content.push(content);
            }
        }

        let combined = all_content.join("\n");
        let estimated_tokens = Self::estimate_tokens(&combined);

        ClaudeMdFileInfo {
            path: expanded_path.to_string_lossy().to_string(),
            exists: true,
            char_count: total_char_count,
            estimated_tokens,
        }
    }

    /// Recursively collect all .md files in a directory tree.
    fn collect_md_files(&self, dir: &Path) -> Vec<PathBuf> {
        let mut md_files = Vec::new();

        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().map_or(false, |ext| ext == "md") {
                    md_files.push(path);
                } else if path.is_dir() {
                    md_files.extend(self.collect_md_files(&path));
                }
            }
        }

        md_files
    }

    /// Get the platform-specific enterprise CLAUDE.md path.
    fn get_enterprise_path() -> String {
        #[cfg(target_os = "windows")]
        {
            "C:\\Program Files\\ClaudeCode\\CLAUDE.md".to_string()
        }
        #[cfg(target_os = "macos")]
        {
            "/Library/Application Support/ClaudeCode/CLAUDE.md".to_string()
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            "/etc/claude-code/CLAUDE.md".to_string()
        }
    }

    /// Read auto memory MEMORY.md file for a project.
    pub fn read_auto_memory_file(&self, project_root: &str) -> ClaudeMdFileInfo {
        let expanded_root = self.expand_tilde(project_root);
        let encoded = super::super::utils::path_decoder::encode_path(&expanded_root.to_string_lossy());
        let memory_path = self.claude_base_path
            .join("projects")
            .join(&encoded)
            .join("memory")
            .join("MEMORY.md");

        match fs::read_to_string(&memory_path) {
            Ok(content) => {
                // Only first 200 lines, matching Claude Code behavior
                let lines: Vec<&str> = content.lines().take(200).collect();
                let truncated = lines.join("\n");
                let char_count = truncated.len();
                let estimated_tokens = Self::estimate_tokens(&truncated);

                ClaudeMdFileInfo {
                    path: memory_path.to_string_lossy().to_string(),
                    exists: true,
                    char_count,
                    estimated_tokens,
                }
            }
            Err(_) => ClaudeMdFileInfo {
                path: memory_path.to_string_lossy().to_string(),
                exists: false,
                char_count: 0,
                estimated_tokens: 0,
            },
        }
    }

    /// Read all potential CLAUDE.md locations for a project.
    pub fn read_all_claude_md_files(&self, project_root: &str) -> ClaudeMdReadResult {
        let mut files = HashMap::new();
        let expanded_project_root = self.expand_tilde(project_root);

        // 1. Enterprise CLAUDE.md (platform-specific path)
        let enterprise_path = Self::get_enterprise_path();
        files.insert("enterprise".to_string(), self.read_claude_md_file(&enterprise_path));

        // 2. User memory: <Claude root>/CLAUDE.md
        let user_memory_path = self.claude_base_path.join("CLAUDE.md");
        files.insert("user".to_string(), self.read_claude_md_file(&user_memory_path.to_string_lossy()));

        // 3. Project memory: ${projectRoot}/CLAUDE.md
        let project_memory_path = expanded_project_root.join("CLAUDE.md");
        files.insert("project".to_string(), self.read_claude_md_file(&project_memory_path.to_string_lossy()));

        // 4. Project memory alt: ${projectRoot}/.claude/CLAUDE.md
        let project_memory_alt_path = expanded_project_root.join(".claude").join("CLAUDE.md");
        files.insert("project-alt".to_string(), self.read_claude_md_file(&project_memory_alt_path.to_string_lossy()));

        // 5. Project rules: ${projectRoot}/.claude/rules/*.md
        let project_rules_path = expanded_project_root.join(".claude").join("rules");
        files.insert("project-rules".to_string(), self.read_directory_md_files(&project_rules_path.to_string_lossy()));

        // 6. Project local: ${projectRoot}/CLAUDE.local.md
        let project_local_path = expanded_project_root.join("CLAUDE.local.md");
        files.insert("project-local".to_string(), self.read_claude_md_file(&project_local_path.to_string_lossy()));

        // 7. User rules: <Claude root>/rules/**/*.md
        let user_rules_path = self.claude_base_path.join("rules");
        files.insert("user-rules".to_string(), self.read_directory_md_files(&user_rules_path.to_string_lossy()));

        // 8. Auto memory: ~/.claude/projects/<encoded>/memory/MEMORY.md
        files.insert("auto-memory".to_string(), self.read_auto_memory_file(project_root));

        ClaudeMdReadResult { files }
    }

    /// Read a specific directory's CLAUDE.md file.
    pub fn read_directory_claude_md(&self, dir_path: &str) -> ClaudeMdFileInfo {
        let expanded_dir_path = self.expand_tilde(dir_path);
        let claude_md_path = expanded_dir_path.join("CLAUDE.md");
        self.read_claude_md_file(&claude_md_path.to_string_lossy())
    }
}

impl Default for ClaudeMdReader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_env() -> (TempDir, ClaudeMdReader) {
        let temp_dir = TempDir::new().unwrap();
        let claude_base = temp_dir.path().join(".claude");
        fs::create_dir_all(&claude_base).unwrap();

        let reader = ClaudeMdReader::with_paths(
            claude_base,
            temp_dir.path().to_path_buf(),
        );
        (temp_dir, reader)
    }

    #[test]
    fn test_expand_tilde() {
        let reader = ClaudeMdReader::new();
        let expanded = reader.expand_tilde("~/test/path");
        assert!(expanded.to_string_lossy().ends_with("test/path"));
    }

    #[test]
    fn test_expand_tilde_no_tilde() {
        let reader = ClaudeMdReader::new();
        let expanded = reader.expand_tilde("/absolute/path");
        assert_eq!(expanded, PathBuf::from("/absolute/path"));
    }

    #[test]
    fn test_estimate_tokens() {
        let content = "Hello world this is a test"; // 27 chars
        let tokens = ClaudeMdReader::estimate_tokens(content);
        assert_eq!(tokens, 6); // 27 / 4 = 6
    }

    #[test]
    fn test_read_claude_md_file_exists() {
        let (temp_dir, reader) = setup_test_env();
        let file_path = temp_dir.path().join("CLAUDE.md");
        let content = "Test content for CLAUDE.md";
        fs::write(&file_path, content).unwrap();

        let result = reader.read_claude_md_file(&file_path.to_string_lossy());
        assert!(result.exists);
        assert_eq!(result.char_count, content.len());
    }

    #[test]
    fn test_read_claude_md_file_not_exists() {
        let reader = ClaudeMdReader::new();
        let result = reader.read_claude_md_file("/nonexistent/path/CLAUDE.md");
        assert!(!result.exists);
        assert_eq!(result.char_count, 0);
    }

    #[test]
    fn test_read_directory_md_files() {
        let (temp_dir, reader) = setup_test_env();
        let rules_dir = temp_dir.path().join("rules");
        fs::create_dir_all(&rules_dir).unwrap();
        fs::write(rules_dir.join("rule1.md"), "Rule 1 content").unwrap();
        fs::write(rules_dir.join("rule2.md"), "Rule 2 content").unwrap();

        let result = reader.read_directory_md_files(&rules_dir.to_string_lossy());
        assert!(result.exists);
        assert!(result.char_count > 0);
    }

    #[test]
    fn test_read_directory_md_files_empty() {
        let (temp_dir, reader) = setup_test_env();
        let empty_dir = temp_dir.path().join("empty");
        fs::create_dir_all(&empty_dir).unwrap();

        let result = reader.read_directory_md_files(&empty_dir.to_string_lossy());
        assert!(!result.exists);
    }

    #[test]
    fn test_read_directory_claude_md() {
        let (temp_dir, reader) = setup_test_env();
        let project_dir = temp_dir.path().join("project");
        fs::create_dir_all(&project_dir).unwrap();
        let content = "Project CLAUDE.md";
        fs::write(project_dir.join("CLAUDE.md"), content).unwrap();

        let result = reader.read_directory_claude_md(&project_dir.to_string_lossy());
        assert!(result.exists);
        assert_eq!(result.char_count, content.len());
    }

    #[test]
    fn test_read_all_claude_md_files() {
        let (temp_dir, reader) = setup_test_env();

        // Create project root
        let project_root = temp_dir.path().join("myproject");
        fs::create_dir_all(&project_root).unwrap();
        fs::write(project_root.join("CLAUDE.md"), "Project instructions").unwrap();

        // Create user CLAUDE.md
        fs::write(temp_dir.path().join(".claude").join("CLAUDE.md"), "User instructions").unwrap();

        let result = reader.read_all_claude_md_files(&project_root.to_string_lossy());

        assert!(result.files.contains_key("enterprise"));
        assert!(result.files.contains_key("user"));
        assert!(result.files.contains_key("project"));
        assert!(result.files.contains_key("project-alt"));
        assert!(result.files.contains_key("project-rules"));
        assert!(result.files.contains_key("project-local"));
        assert!(result.files.contains_key("user-rules"));
        assert!(result.files.contains_key("auto-memory"));

        // Check project file was found
        assert!(result.files.get("project").unwrap().exists);
        assert!(result.files.get("user").unwrap().exists);
    }
}