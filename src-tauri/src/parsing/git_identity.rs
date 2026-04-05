//! GitIdentityResolver service - Resolves git repository identity from project paths.
//!
//! Responsibilities:
//! - Detect if a path is inside a git worktree vs main repository
//! - Extract the main repository path from worktree's .git file
//! - Get git remote URL for repository identity
//! - Build consistent repository identity across all worktrees
//!
//! Git worktree detection:
//! - Main repo: .git is a directory
//! - Worktree: .git is a file containing "gitdir: /path/to/main/.git/worktrees/<name>"

use crate::constants::*;
use crate::types::domain::{RepositoryIdentity, WorktreeSource};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};
use std::fs;

/// GitIdentityResolver resolves repository identity from project paths.
pub struct GitIdentityResolver;

impl GitIdentityResolver {
    /// Create a new GitIdentityResolver instance.
    pub fn new() -> Self {
        Self
    }

    /// Resolve repository identity from a project path.
    ///
    /// Algorithm:
    /// 1. Check if path/.git exists on filesystem
    /// 2. If .git is a file (worktree), read gitdir to find main repo
    /// 3. If .git is a directory (main repo), use it directly
    /// 4. Extract remote URL from .git/config
    /// 5. Build RepositoryIdentity with consistent ID
    /// 6. FALLBACK: If path doesn't exist, use heuristics based on path patterns
    pub fn resolve_identity(&self, project_path: &str) -> Option<RepositoryIdentity> {
        let git_path = PathBuf::from(project_path).join(".git");

        // First, try filesystem-based resolution
        if git_path.exists() {
            let metadata = fs::metadata(&git_path).ok()?;

            let main_git_dir = if metadata.is_file() {
                // This is a worktree - parse the .git file to find main repo
                let git_file_content = fs::read_to_string(&git_path).ok()?.trim().to_string();
                let git_dir = self.parse_gitdir(&git_file_content)?;

                // Handle relative paths in gitdir
                let worktree_git_dir = if Path::new(&git_dir).is_absolute() {
                    PathBuf::from(git_dir)
                } else {
                    PathBuf::from(project_path).join(git_dir)
                };

                self.extract_main_git_dir(&worktree_git_dir.to_string_lossy())
            } else if metadata.is_dir() {
                git_path.clone()
            } else {
                return self.resolve_identity_from_path(project_path);
            };

            // Normalize the path to handle symlinks
            let normalized_git_dir = fs::canonicalize(&main_git_dir).unwrap_or(main_git_dir);

            // Extract remote URL from config
            let remote_url = self.get_remote_url(&normalized_git_dir);

            // Generate consistent repository ID
            let repo_id = self.generate_repo_id(remote_url.as_deref(), &normalized_git_dir.to_string_lossy());

            // Extract repository name
            let repo_name = self.extract_repo_name(&normalized_git_dir.to_string_lossy());

            Some(RepositoryIdentity {
                id: repo_id,
                remote_url,
                main_git_dir: normalized_git_dir.to_string_lossy().to_string(),
                name: repo_name,
            })
        } else {
            // Fallback: path doesn't exist, use heuristic resolution
            self.resolve_identity_from_path(project_path)
        }
    }

    /// Parse gitdir from .git file content.
    fn parse_gitdir(&self, content: &str) -> Option<String> {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("gitdir:") {
                let dir = trimmed["gitdir:".len()..].trim();
                return Some(dir.to_string());
            }
        }
        None
    }

    /// Extract the main .git directory path from a worktree's gitdir.
    fn extract_main_git_dir(&self, worktree_git_dir: &str) -> PathBuf {
        let path = Path::new(worktree_git_dir);
        let parts: Vec<_> = path.components().collect();

        // Find "worktrees" in the path
        for (i, component) in parts.iter().enumerate() {
            if let Component::Normal(os_str) = component {
                if os_str.to_string_lossy() == WORKTREES_DIR {
                    // Return everything up to and including .git
                    return parts[..i].iter().collect();
                }
            }
        }

        // Fallback: try to find .git in the path
        let path_str = worktree_git_dir;
        if let Some(pos) = path_str.rfind(".git") {
            return PathBuf::from(&path_str[..pos + 4]);
        }

        PathBuf::from(worktree_git_dir)
    }

    /// Get git remote URL from a repository's config file.
    fn get_remote_url(&self, git_dir: &Path) -> Option<String> {
        let config_path = git_dir.join("config");

        if !config_path.exists() {
            return None;
        }

        let config_content = fs::read_to_string(&config_path).ok()?;
        self.parse_remote_url(&config_content)
    }

    /// Parse remote URL from git config content.
    fn parse_remote_url(&self, content: &str) -> Option<String> {
        let mut in_origin_remote = false;

        for line in content.lines() {
            let trimmed = line.trim();

            // Check for remote "origin" section
            if trimmed == r#"[remote "origin"]"# {
                in_origin_remote = true;
                continue;
            }

            // Check for new section (exit origin remote)
            if trimmed.starts_with('[') && in_origin_remote {
                break;
            }

            // Look for url = ... in origin remote section
            if in_origin_remote && trimmed.starts_with("url") {
                if let Some(eq_pos) = trimmed.find('=') {
                    let url = trimmed[eq_pos + 1..].trim();
                    if !url.is_empty() {
                        return Some(url.to_string());
                    }
                }
            }
        }

        None
    }

    /// Generate consistent repository ID.
    /// Uses the LOCAL DIRECTORY NAME as the primary identifier.
    fn generate_repo_id(&self, remote_url: Option<&str>, main_git_dir_or_name: &str) -> String {
        let identity = if main_git_dir_or_name.contains('/') || main_git_dir_or_name.contains('\\') {
            if remote_url.is_some() {
                // Has remote → use dir name (allows worktree grouping)
                let parent_dir = Path::new(main_git_dir_or_name)
                    .parent()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| main_git_dir_or_name.to_string());
                parent_dir
            } else {
                // No remote → use full path to distinguish same-named repos
                let path = if main_git_dir_or_name.ends_with(".git") {
                    Path::new(main_git_dir_or_name)
                        .parent()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| main_git_dir_or_name.to_string())
                } else {
                    main_git_dir_or_name.to_string()
                };
                path
            }
        } else {
            // It's already just a name (from path-based resolution fallback)
            main_git_dir_or_name.to_string()
        };

        // Normalize and generate hash
        let normalized = identity.to_lowercase();

        // Generate SHA-256 hash and take first 12 characters
        let mut hasher = Sha256::new();
        hasher.update(normalized.as_bytes());
        let hash = hasher.finalize();
        format!("{:x}", hash)[..12].to_string()
    }

    /// Extract repository name from git directory path.
    fn extract_repo_name(&self, main_git_dir: &str) -> String {
        // Always use local directory name for consistency
        // /Users/username/projectname/.git -> projectname
        Path::new(main_git_dir)
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string())
    }

    /// Fallback: Resolve repository identity from path patterns.
    fn resolve_identity_from_path(&self, project_path: &str) -> Option<RepositoryIdentity> {
        let repo_name = self.extract_repo_name_from_path(project_path)?;

        // Generate ID from full path (since no remote URL)
        let repo_id = self.generate_repo_id(None, project_path);

        Some(RepositoryIdentity {
            id: repo_id,
            remote_url: None,
            main_git_dir: repo_name.clone(),
            name: repo_name,
        })
    }

    /// Extract repository name from path using heuristics.
    fn extract_repo_name_from_path(&self, project_path: &str) -> Option<String> {
        let parts: Vec<&str> = project_path
            .split(|c| c == '/' || c == '\\')
            .filter(|s| !s.is_empty())
            .collect();

        if parts.is_empty() {
            return None;
        }

        // Pattern 1: /.cursor/worktrees/{repo}/{worktree-name}
        if let Some(idx) = parts.iter().position(|&p| p == CURSOR_DIR) {
            if idx + 2 < parts.len() && parts[idx + 1] == WORKTREES_DIR {
                return Some(parts[idx + 2].to_string());
            }
        }

        // Pattern 2: /vibe-kanban/worktrees/{issue-branch}/{repo}
        if let Some(idx) = parts.iter().position(|&p| p == VIBE_KANBAN_DIR) {
            if idx + 1 < parts.len() && parts[idx + 1] == WORKTREES_DIR {
                return Some(parts[parts.len() - 1].to_string());
            }
        }

        // Pattern 3: /conductor/workspaces/{repo}/{subpath}
        if let Some(idx) = parts.iter().position(|&p| p == CONDUCTOR_DIR) {
            if idx + 2 < parts.len() && parts[idx + 1] == WORKSPACES_DIR {
                return Some(parts[idx + 2].to_string());
            }
        }

        // Pattern 4: /.auto-claude/worktrees/tasks/{task-id}
        if let Some(idx) = parts.iter().position(|&p| p == AUTO_CLAUDE_DIR) {
            if idx > 0 && idx + 1 < parts.len() && parts[idx + 1] == WORKTREES_DIR {
                return Some(parts[idx - 1].to_string());
            }
        }

        // Pattern 5: /.21st/worktrees/{id}/{name}
        if let Some(idx) = parts.iter().position(|&p| p == TWENTYFIRST_DIR) {
            if idx > 0 && idx + 1 < parts.len() && parts[idx + 1] == WORKTREES_DIR {
                return Some(parts[idx - 1].to_string());
            }
        }

        // Pattern 6: /.claude-worktrees/{repo}/{name}
        if let Some(idx) = parts.iter().position(|&p| p == CLAUDE_WORKTREES_DIR) {
            if idx + 1 < parts.len() {
                return Some(parts[idx + 1].to_string());
            }
        }

        // Pattern 7: /.ccswitch/worktrees/{repo}/{name}
        if let Some(idx) = parts.iter().position(|&p| p == CCSWITCH_DIR) {
            if idx + 2 < parts.len() && parts[idx + 1] == WORKTREES_DIR {
                return Some(parts[idx + 2].to_string());
            }
        }

        // Default: use the last component
        Some(parts[parts.len() - 1].to_string())
    }

    /// Determine if a path is a worktree (vs main repo).
    pub fn is_worktree(&self, project_path: &str) -> bool {
        let parts: Vec<&str> = project_path
            .split(|c| c == '/' || c == '\\')
            .filter(|s| !s.is_empty())
            .collect();

        // Check for known worktree patterns
        if parts.iter().any(|&p| p == CURSOR_DIR) && parts.iter().any(|&p| p == WORKTREES_DIR) {
            return true;
        }
        if parts.iter().any(|&p| p == VIBE_KANBAN_DIR) && parts.iter().any(|&p| p == WORKTREES_DIR) {
            return true;
        }
        if parts.iter().any(|&p| p == AUTO_CLAUDE_DIR) && parts.iter().any(|&p| p == WORKTREES_DIR) {
            return true;
        }
        if parts.iter().any(|&p| p == TWENTYFIRST_DIR) && parts.iter().any(|&p| p == WORKTREES_DIR) {
            return true;
        }
        if parts.iter().any(|&p| p == CLAUDE_WORKTREES_DIR) {
            return true;
        }
        if parts.iter().any(|&p| p == CCSWITCH_DIR) && parts.iter().any(|&p| p == WORKTREES_DIR) {
            return true;
        }
        // Subpaths in conductor/workspaces are worktrees
        if let Some(conductor_idx) = parts.iter().position(|&p| p == CONDUCTOR_DIR) {
            if conductor_idx + 3 < parts.len() && parts[conductor_idx + 1] == WORKSPACES_DIR {
                return true;
            }
        }

        // Fallback: check filesystem
        let git_path = PathBuf::from(project_path).join(".git");
        if git_path.exists() {
            return git_path.is_file();
        }

        false
    }

    /// Get the git branch for a worktree.
    pub fn get_branch(&self, project_path: &str) -> Option<String> {
        let git_path = PathBuf::from(project_path).join(".git");

        if !git_path.exists() {
            return None;
        }

        let metadata = fs::metadata(&git_path).ok()?;
        let head_path = if metadata.is_file() {
            // Worktree - read .git file to find the HEAD location
            let content = fs::read_to_string(&git_path).ok()?;
            let git_dir = self.parse_gitdir(&content)?;
            PathBuf::from(git_dir).join("HEAD")
        } else {
            git_path.join("HEAD")
        };

        if !head_path.exists() {
            return None;
        }

        let head_content = fs::read_to_string(&head_path).ok()?.trim().to_string();

        // Check if HEAD is a symbolic ref (branch)
        if head_content.starts_with("ref: refs/heads/") {
            return Some(head_content["ref: refs/heads/".len()..].to_string());
        }

        // HEAD is detached (commit hash)
        Some("detached HEAD".to_string())
    }

    /// Detect the worktree source based on path patterns.
    pub fn detect_worktree_source(&self, project_path: &str) -> WorktreeSource {
        let parts: Vec<&str> = project_path
            .split(|c| c == '/' || c == '\\')
            .filter(|s| !s.is_empty())
            .collect();

        if parts.iter().any(|&p| p == VIBE_KANBAN_DIR) && parts.iter().any(|&p| p == WORKTREES_DIR) {
            return WorktreeSource::VibeKanban;
        }

        if parts.iter().any(|&p| p == CONDUCTOR_DIR) && parts.iter().any(|&p| p == WORKSPACES_DIR) {
            return WorktreeSource::Conductor;
        }

        if parts.iter().any(|&p| p == AUTO_CLAUDE_DIR) && parts.iter().any(|&p| p == WORKTREES_DIR) {
            return WorktreeSource::AutoClaude;
        }

        if parts.iter().any(|&p| p == TWENTYFIRST_DIR) && parts.iter().any(|&p| p == WORKTREES_DIR) {
            return WorktreeSource::TwentyFirst;
        }

        if parts.iter().any(|&p| p == CLAUDE_WORKTREES_DIR) {
            return WorktreeSource::ClaudeDesktop;
        }

        if parts.iter().any(|&p| p == CCSWITCH_DIR) && parts.iter().any(|&p| p == WORKTREES_DIR) {
            return WorktreeSource::Ccswitch;
        }

        // Check if it's a standard git repo
        let git_path = PathBuf::from(project_path).join(".git");
        if git_path.exists() {
            return WorktreeSource::Git;
        }

        WorktreeSource::Unknown
    }

    /// Get the display name for a worktree based on its source.
    pub fn get_worktree_display_name(
        &self,
        project_path: &str,
        source: &WorktreeSource,
        branch: Option<&str>,
        is_main_worktree: bool,
    ) -> String {
        let parts: Vec<&str> = project_path
            .split(|c| c == '/' || c == '\\')
            .filter(|s| !s.is_empty())
            .collect();

        match source {
            WorktreeSource::VibeKanban => {
                // Pattern: vibe-kanban/worktrees/{issue-branch}/{repo}
                if let Some(idx) = parts.iter().position(|&p| p == WORKTREES_DIR) {
                    if idx + 1 < parts.len() {
                        return parts[idx + 1].to_string();
                    }
                }
            }
            WorktreeSource::Conductor => {
                // Pattern: conductor/workspaces/{repo}/{workspace}
                if let Some(idx) = parts.iter().position(|&p| p == WORKSPACES_DIR) {
                    if idx + 2 < parts.len() {
                        return parts[idx + 2].to_string();
                    }
                }
            }
            WorktreeSource::AutoClaude => {
                // Pattern: .auto-claude/worktrees/tasks/{task-id}
                if let Some(idx) = parts.iter().position(|&p| p == TASKS_DIR) {
                    if idx + 1 < parts.len() {
                        return parts[idx + 1].to_string();
                    }
                }
                return parts.last().map(|s| s.to_string()).unwrap_or_default();
            }
            WorktreeSource::TwentyFirst => {
                // Pattern: .21st/worktrees/{id}/{name with [bracket-id]}
                if let Some(last) = parts.last() {
                    // Extract content from square brackets
                    if let (Some(start), Some(end)) = (last.find('['), last.find(']')) {
                        if end > start + 1 {
                            return last[start + 1..end].to_string();
                        }
                    }
                    return last.to_string();
                }
            }
            WorktreeSource::ClaudeDesktop => {
                // Pattern: .claude-worktrees/{repo}/{name}
                if let Some(idx) = parts.iter().position(|&p| p == CLAUDE_WORKTREES_DIR) {
                    if idx + 2 < parts.len() {
                        return parts[idx + 2].to_string();
                    }
                }
            }
            WorktreeSource::Ccswitch => {
                // Pattern: .ccswitch/worktrees/{repo}/{name}
                if let Some(ccswitch_idx) = parts.iter().position(|&p| p == CCSWITCH_DIR) {
                    let remaining = &parts[ccswitch_idx..];
                    if let Some(worktrees_idx) = remaining.iter().position(|&p| p == WORKTREES_DIR) {
                        if worktrees_idx + 2 < remaining.len() {
                            return remaining[worktrees_idx + 2].to_string();
                        }
                    }
                }
            }
            WorktreeSource::Git => {
                // Standard git worktree - use branch or path-based name
                if is_main_worktree {
                    return branch.unwrap_or("main").to_string();
                }
                // For non-main git worktrees, try to get the worktree name from .git file
                if let Some(name) = self.get_git_worktree_name(project_path) {
                    return name;
                }
                return branch.map(|s| s.to_string())
                    .or_else(|| parts.last().map(|s| s.to_string()))
                    .unwrap_or_default();
            }
            WorktreeSource::Unknown => {
                return parts.last().map(|s| s.to_string()).unwrap_or_else(|| "unknown".to_string());
            }
        }

        // Fallback for any case that didn't return
        branch.map(|s| s.to_string())
            .or_else(|| parts.last().map(|s| s.to_string()))
            .unwrap_or_else(|| "unknown".to_string())
    }

    /// Get the worktree name from git's internal tracking.
    fn get_git_worktree_name(&self, project_path: &str) -> Option<String> {
        let git_path = PathBuf::from(project_path).join(".git");

        if !git_path.exists() || !git_path.is_file() {
            return None;
        }

        let content = fs::read_to_string(&git_path).ok()?;
        let git_dir = self.parse_gitdir(&content)?;

        // gitdir: /main/.git/worktrees/my-worktree-name
        let parts: Vec<&str> = git_dir.split(|c| c == '/' || c == '\\').collect();
        if let Some(idx) = parts.iter().position(|&p| p == WORKTREES_DIR) {
            if idx + 1 < parts.len() {
                return Some(parts[idx + 1].to_string());
            }
        }

        None
    }
}

impl Default for GitIdentityResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gitdir() {
        let resolver = GitIdentityResolver::new();
        let content = "gitdir: /path/to/main/.git/worktrees/my-worktree";
        let result = resolver.parse_gitdir(content);
        assert_eq!(result, Some("/path/to/main/.git/worktrees/my-worktree".to_string()));
    }

    #[test]
    fn test_parse_gitdir_with_whitespace() {
        let resolver = GitIdentityResolver::new();
        let content = "gitdir:   /path/to/main/.git/worktrees/my-worktree  ";
        let result = resolver.parse_gitdir(content);
        assert_eq!(result, Some("/path/to/main/.git/worktrees/my-worktree".to_string()));
    }

    #[test]
    fn test_extract_main_git_dir() {
        let resolver = GitIdentityResolver::new();
        let worktree_git_dir = "/Users/test/project/.git/worktrees/feature-branch";
        let result = resolver.extract_main_git_dir(worktree_git_dir);
        assert_eq!(result, PathBuf::from("/Users/test/project/.git"));
    }

    #[test]
    fn test_generate_repo_id_consistency() {
        let resolver = GitIdentityResolver::new();

        // Same remote URL and main git dir should produce the same ID
        let id1 = resolver.generate_repo_id(
            Some("https://github.com/user/repo.git"),
            "/Users/test/project/.git"
        );
        let id2 = resolver.generate_repo_id(
            Some("https://github.com/user/repo.git"),
            "/Users/test/project/.git"
        );
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_generate_repo_id_different_paths() {
        let resolver = GitIdentityResolver::new();

        // Without remote URL, different paths should produce different IDs
        let id1 = resolver.generate_repo_id(None, "/Users/test/project1/.git");
        let id2 = resolver.generate_repo_id(None, "/Users/test/project2/.git");
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_extract_repo_name() {
        let resolver = GitIdentityResolver::new();
        let name = resolver.extract_repo_name("/Users/test/my-project/.git");
        assert_eq!(name, "my-project");
    }

    #[test]
    fn test_extract_repo_name_from_path_cursor() {
        let resolver = GitIdentityResolver::new();
        let path = "/Users/test/.cursor/worktrees/my-repo/feature-branch";
        let name = resolver.extract_repo_name_from_path(path);
        assert_eq!(name, Some("my-repo".to_string()));
    }

    #[test]
    fn test_extract_repo_name_from_path_vibe_kanban() {
        let resolver = GitIdentityResolver::new();
        let path = "/tmp/vibe-kanban/worktrees/issue-123/my-repo";
        let name = resolver.extract_repo_name_from_path(path);
        assert_eq!(name, Some("my-repo".to_string()));
    }

    #[test]
    fn test_extract_repo_name_from_path_default() {
        let resolver = GitIdentityResolver::new();
        let path = "/Users/test/my-project";
        let name = resolver.extract_repo_name_from_path(path);
        assert_eq!(name, Some("my-project".to_string()));
    }

    #[test]
    fn test_is_worktree_cursor() {
        let resolver = GitIdentityResolver::new();
        let path = "/Users/test/.cursor/worktrees/my-repo/feature";
        assert!(resolver.is_worktree(path));
    }

    #[test]
    fn test_is_worktree_vibe_kanban() {
        let resolver = GitIdentityResolver::new();
        let path = "/tmp/vibe-kanban/worktrees/issue-123/repo";
        assert!(resolver.is_worktree(path));
    }

    #[test]
    fn test_is_worktree_not_worktree() {
        let resolver = GitIdentityResolver::new();
        let path = "/Users/test/my-project";
        // This will check filesystem, so it might be false or true depending on actual .git
        // Just test that the method doesn't panic
        let _ = resolver.is_worktree(path);
    }

    #[test]
    fn test_detect_worktree_source_vibe_kanban() {
        let resolver = GitIdentityResolver::new();
        let path = "/tmp/vibe-kanban/worktrees/issue-123/repo";
        let source = resolver.detect_worktree_source(path);
        assert_eq!(source, WorktreeSource::VibeKanban);
    }

    #[test]
    fn test_detect_worktree_source_conductor() {
        let resolver = GitIdentityResolver::new();
        let path = "/Users/test/conductor/workspaces/repo/workspace";
        let source = resolver.detect_worktree_source(path);
        assert_eq!(source, WorktreeSource::Conductor);
    }

    #[test]
    fn test_detect_worktree_source_claude_desktop() {
        let resolver = GitIdentityResolver::new();
        let path = "/Users/test/.claude-worktrees/repo/workspace";
        let source = resolver.detect_worktree_source(path);
        assert_eq!(source, WorktreeSource::ClaudeDesktop);
    }

    #[test]
    fn test_parse_remote_url() {
        let resolver = GitIdentityResolver::new();
        let config = r#"
[core]
        repositoryformatversion = 0
[remote "origin"]
        url = https://github.com/user/repo.git
        fetch = +refs/heads/*:refs/remotes/origin/*
[branch "main"]
        remote = origin
"#;
        let result = resolver.parse_remote_url(config);
        assert_eq!(result, Some("https://github.com/user/repo.git".to_string()));
    }

    #[test]
    fn test_parse_remote_url_no_origin() {
        let resolver = GitIdentityResolver::new();
        let config = r#"
[core]
        repositoryformatversion = 0
[branch "main"]
        remote = origin
"#;
        let result = resolver.parse_remote_url(config);
        assert_eq!(result, None);
    }
}