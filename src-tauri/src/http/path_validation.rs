//! 路径安全验证模块。
//!
//! 移植自 Electron `src/main/utils/pathValidation.ts`。
//! 提供 5 层防护：绝对路径、路径规范化、允许目录、敏感模式、符号链接。

use once_cell::sync::Lazy;
use regex::Regex;
use std::path::{Path, PathBuf};

use crate::utils::get_default_claude_base_path;

// ---------------------------------------------------------------------------
// 敏感文件模式（23 条正则，移植自 Electron SENSITIVE_PATTERNS）
// ---------------------------------------------------------------------------

static SENSITIVE_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    let patterns: &[&str] = &[
        // SSH keys and config
        r"[/\\]\.ssh[/\\]",
        // AWS credentials
        r"[/\\]\.aws[/\\]",
        // GCP credentials
        r"[/\\]\.config[/\\]gcloud[/\\]",
        // Azure credentials
        r"[/\\]\.azure[/\\]",
        // Environment files (anywhere in path)
        r"[/\\]\.env($|\.)",
        // Git credentials
        r"[/\\]\.git-credentials$",
        r"[/\\]\.gitconfig$",
        // NPM tokens
        r"[/\\]\.npmrc$",
        // Docker credentials
        r"[/\\]\.docker[/\\]config\.json$",
        // Kubernetes config
        r"[/\\]\.kube[/\\]config$",
        // Password files
        r"[/\\]\.password",
        // Secret files
        r"[/\\]\.secret",
        // Private keys
        r"[/\\]id_rsa$",
        r"[/\\]id_ed25519$",
        r"[/\\]id_ecdsa$",
        r"[/\\][^/\\]*\.pem$",
        r"[/\\][^/\\]*\.key$",
        // System files
        r"^/etc/passwd$",
        r"^/etc/shadow$",
        // Credentials in filename
        r"credentials\.json$",
        r"secrets\.json$",
        r"tokens\.json$",
    ];
    patterns
        .iter()
        .map(|p| Regex::new(p).expect("invalid sensitive pattern"))
        .collect()
});

// ---------------------------------------------------------------------------
// 纯内存路径规范化
// ---------------------------------------------------------------------------

/// 纯内存路径规范化 -- 仅消除 `.` 和 `..` 组件，不涉及文件系统 I/O。
///
/// 对应 Electron 的 `path.normalize()` / `path.resolve()` 中去掉 traversal 段的部分。
/// 使用此函数而非 `std::fs::canonicalize` 以避免 TOCTOU 竞态。
fn normalize_components(p: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in p.components() {
        match component {
            std::path::Component::CurDir => { /* skip "." */ }
            std::path::Component::ParentDir => {
                // pop() is safe: on a root-only path ("/") it is a no-op.
                result.pop();
            }
            std::path::Component::Normal(os_str) => {
                result.push(os_str);
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                // Re-start from the root/prefix.
                result.clear();
                result.push(component.as_os_str());
            }
        }
    }
    // If the path was empty (e.g. "foo/.."), produce ".".
    if result.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        result
    }
}

// ---------------------------------------------------------------------------
// 路径包含关系检查
// ---------------------------------------------------------------------------

/// 检查路径包含关系（纯内存操作）。
///
/// 对应 Electron 的 `isPathWithinRoot()`。
/// 使用字符串比较 + 分隔符后缀避免前缀误判
/// （例如 `/app` 不会匹配 `/application`）。
pub fn is_path_contained(full_path: &Path, base_path: &Path) -> bool {
    let normalized_full = normalize_components(full_path);
    let normalized_base = normalize_components(base_path);

    if normalized_full == normalized_base {
        return true;
    }

    // String comparison with separator suffix to avoid prefix false-positives.
    // (Path::join("/") on Unix treats "/" as absolute and replaces the path.)
    let full_str = normalized_full.to_string_lossy();
    let base_with_sep = format!("{}/", normalized_base.to_string_lossy());
    full_str.starts_with(&base_with_sep)
}

// ---------------------------------------------------------------------------
// 敏感模式匹配
// ---------------------------------------------------------------------------

/// 检查路径是否匹配敏感文件模式。
fn matches_sensitive_pattern(path: &str) -> bool {
    SENSITIVE_PATTERNS.iter().any(|re| re.is_match(path))
}

// ---------------------------------------------------------------------------
// 允许目录检查
// ---------------------------------------------------------------------------

/// 检查路径是否在允许的目录内（项目目录或 `~/.claude`）。
fn is_path_within_allowed_dirs(normalized: &Path, project_root: Option<&Path>) -> bool {
    // Always allow access to ~/.claude for session data.
    let claude_dir = get_default_claude_base_path();
    if is_path_contained(normalized, &claude_dir) {
        return true;
    }

    // If project path provided, allow access within project.
    if let Some(project) = project_root {
        if is_path_contained(normalized, project) {
            return true;
        }
    }

    false
}

// ---------------------------------------------------------------------------
// 符号链接解析（仅第 5 层使用）
// ---------------------------------------------------------------------------

/// 解析符号链接。
///
/// 仅在第 5 层防护中使用，不用于前 4 层（避免 TOCTOU）。
/// 对应 Electron 的 `fs.realpathSync.native()`。
fn resolve_real_path(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

// ---------------------------------------------------------------------------
// 验证结果
// ---------------------------------------------------------------------------

/// 路径验证结果。
#[derive(Debug, Clone)]
pub struct PathValidationResult {
    pub valid: bool,
    pub error: Option<String>,
    pub normalized_path: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// 主入口：5 层防护验证
// ---------------------------------------------------------------------------

/// 主入口：5 层防护验证。
///
/// 1. 路径必须非空且为字符串
/// 2. 路径必须是绝对路径（含 `~` 展开后）
/// 3. 路径必须在允许目录内（项目目录或 `~/.claude`）
/// 4. 路径不能匹配敏感文件模式
/// 5. 符号链接解析后重新检查 3 & 4
pub fn validate_file_path(
    file_path: &str,
    project_root: Option<&str>,
) -> PathValidationResult {
    // Layer 1: Must be a non-empty string.
    if file_path.is_empty() {
        return PathValidationResult {
            valid: false,
            error: Some("Invalid file path".to_string()),
            normalized_path: None,
        };
    }

    // Expand ~ to home directory.
    let expanded = if file_path.starts_with('~') {
        match dirs::home_dir() {
            Some(home) => home.join(&file_path[1..]),
            None => PathBuf::from(file_path),
        }
    } else {
        PathBuf::from(file_path)
    };

    // Layer 2: Path must be absolute.
    let normalized = normalize_components(&expanded);
    if !normalized.is_absolute() {
        return PathValidationResult {
            valid: false,
            error: Some("Path must be absolute".to_string()),
            normalized_path: None,
        };
    }

    let normalized_str = normalized.to_string_lossy();

    // Layer 4 (before 3 — match Electron order): sensitive pattern check.
    if matches_sensitive_pattern(&normalized_str) {
        return PathValidationResult {
            valid: false,
            error: Some("Access to sensitive files is not allowed".to_string()),
            normalized_path: None,
        };
    }

    // Layer 3: Must be within allowed directories.
    let project_path = project_root.map(Path::new);
    if !is_path_within_allowed_dirs(&normalized, project_path) {
        return PathValidationResult {
            valid: false,
            error: Some(
                "Path is outside allowed directories (project or Claude root)".to_string(),
            ),
            normalized_path: None,
        };
    }

    // Layer 5: Symlink resolution — re-check containment and sensitivity.
    if let Some(real_path) = resolve_real_path(&normalized) {
        let real_str = real_path.to_string_lossy();

        if matches_sensitive_pattern(&real_str) {
            return PathValidationResult {
                valid: false,
                error: Some("Access to sensitive files is not allowed".to_string()),
                normalized_path: None,
            };
        }

        let real_project = project_root.and_then(|p| {
            resolve_real_path(Path::new(p)).or_else(|| {
                let norm = normalize_components(Path::new(p));
                if norm.is_absolute() {
                    Some(norm)
                } else {
                    None
                }
            })
        });

        if !is_path_within_allowed_dirs(&real_path, real_project.as_deref()) {
            return PathValidationResult {
                valid: false,
                error: Some(
                    "Path is outside allowed directories (project or Claude root)".to_string(),
                ),
                normalized_path: None,
            };
        }
    }

    PathValidationResult {
        valid: true,
        error: None,
        normalized_path: Some(normalized),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        dirs::home_dir().expect("home dir")
    }

    #[test]
    fn normalize_basic() {
        assert_eq!(
            normalize_components(Path::new("/a/b/../c")),
            PathBuf::from("/a/c")
        );
        assert_eq!(
            normalize_components(Path::new("/a/./b")),
            PathBuf::from("/a/b")
        );
        assert_eq!(
            normalize_components(Path::new("/a/b/../../c")),
            PathBuf::from("/c")
        );
    }

    #[test]
    fn normalize_noop() {
        assert_eq!(
            normalize_components(Path::new("/a/b/c")),
            PathBuf::from("/a/b/c")
        );
    }

    #[test]
    fn normalize_traversal_beyond_root_yields_root() {
        // Going above root should still produce a valid absolute path.
        let result = normalize_components(Path::new("/../a"));
        assert!(result.is_absolute());
    }

    #[test]
    fn is_path_contained_direct() {
        let base = Path::new("/home/user/project");
        let child = Path::new("/home/user/project/src/main.rs");
        assert!(is_path_contained(child, base));
        assert!(is_path_contained(base, base));
    }

    #[test]
    fn is_path_contained_rejects_prefix_false_positive() {
        // /app should NOT match /application.
        assert!(!is_path_contained(Path::new("/application"), Path::new("/app")));
    }

    #[test]
    fn is_path_contained_independent() {
        assert!(!is_path_contained(
            Path::new("/other/project/file.rs"),
            Path::new("/home/user/project")
        ));
    }

    #[test]
    fn validate_empty_path() {
        let result = validate_file_path("", None);
        assert!(!result.valid);
        assert_eq!(result.error.as_deref(), Some("Invalid file path"));
    }

    #[test]
    fn validate_relative_path() {
        let result = validate_file_path("relative/path.txt", None);
        assert!(!result.valid);
        assert_eq!(result.error.as_deref(), Some("Path must be absolute"));
    }

    #[test]
    fn validate_sensitive_ssh_key() {
        let ssh_path = format!("{}/.ssh/id_rsa", home().display());
        let result = validate_file_path(&ssh_path, None);
        assert!(!result.valid);
        assert_eq!(
            result.error.as_deref(),
            Some("Access to sensitive files is not allowed")
        );
    }

    #[test]
    fn validate_sensitive_env_file() {
        let env_path = "/home/user/project/.env".to_string();
        let result = validate_file_path(&env_path, Some("/home/user/project"));
        assert!(!result.valid);
        assert_eq!(
            result.error.as_deref(),
            Some("Access to sensitive files is not allowed")
        );
    }

    #[test]
    fn validate_sensitive_etc_passwd() {
        let result = validate_file_path("/etc/passwd", None);
        assert!(!result.valid);
        assert_eq!(
            result.error.as_deref(),
            Some("Access to sensitive files is not allowed")
        );
    }

    #[test]
    fn validate_allows_claude_dir() {
        let claude_base = get_default_claude_base_path();
        let session_file = claude_base.join("projects").join("-U-n").join("s1.jsonl");
        let result = validate_file_path(&session_file.to_string_lossy(), None);
        // May fail if the file does not exist, but layers 1-4 should pass.
        // Layer 5 (symlink) is skipped when file doesn't exist.
        assert!(result.valid, "expected valid, got error: {:?}", result.error);
    }

    #[test]
    fn validate_allows_project_dir() {
        let result = validate_file_path("/home/user/project/src/main.rs", Some("/home/user/project"));
        // Layers 1-4 should pass; layer 5 depends on fs.
        assert!(result.valid, "expected valid, got error: {:?}", result.error);
    }

    #[test]
    fn validate_rejects_outside_dirs() {
        let result = validate_file_path("/etc/hostname", None);
        assert!(!result.valid);
        assert!(result
            .error
            .as_ref()
            .unwrap()
            .contains("outside allowed directories"));
    }

    #[test]
    fn validate_tilde_expansion() {
        let result = validate_file_path("~/some/random/path.txt", None);
        // After expansion it becomes /home/user/some/random/path.txt,
        // which is outside allowed dirs.
        assert!(!result.valid);
        // Not "Path must be absolute" -- it should expand ~ first.
        assert_ne!(result.error.as_deref(), Some("Path must be absolute"));
    }

    #[test]
    fn validate_traversal_in_path() {
        let project = "/home/user/project";
        let traversal = format!("{project}/../../../etc/hostname");
        let result = validate_file_path(&traversal, Some(project));
        assert!(!result.valid);
    }

    #[test]
    fn all_23_sensitive_patterns_compiled() {
        assert_eq!(SENSITIVE_PATTERNS.len(), 22); // Electron has 22 patterns
    }

    #[test]
    fn sensitive_pattern_matches() {
        assert!(matches_sensitive_pattern("/home/u/.ssh/id_rsa"));
        assert!(matches_sensitive_pattern("/home/u/.aws/credentials"));
        assert!(matches_sensitive_pattern("/home/u/.config/gcloud/abc.json"));
        assert!(matches_sensitive_pattern("/home/u/.azure/credentials"));
        assert!(matches_sensitive_pattern("/home/u/project/.env"));
        assert!(matches_sensitive_pattern("/home/u/project/.env.local"));
        assert!(matches_sensitive_pattern("/home/u/.git-credentials"));
        assert!(matches_sensitive_pattern("/home/u/.gitconfig"));
        assert!(matches_sensitive_pattern("/home/u/.npmrc"));
        assert!(matches_sensitive_pattern("/home/u/.docker/config.json"));
        assert!(matches_sensitive_pattern("/home/u/.kube/config"));
        assert!(matches_sensitive_pattern("/home/u/.password"));
        assert!(matches_sensitive_pattern("/home/u/.secret"));
        assert!(matches_sensitive_pattern("/home/u/id_rsa"));
        assert!(matches_sensitive_pattern("/home/u/id_ed25519"));
        assert!(matches_sensitive_pattern("/home/u/id_ecdsa"));
        assert!(matches_sensitive_pattern("/home/u/cert.pem"));
        assert!(matches_sensitive_pattern("/home/u/server.key"));
        assert!(matches_sensitive_pattern("/etc/passwd"));
        assert!(matches_sensitive_pattern("/etc/shadow"));
        assert!(matches_sensitive_pattern("/app/credentials.json"));
        assert!(matches_sensitive_pattern("/app/secrets.json"));
        assert!(matches_sensitive_pattern("/app/tokens.json"));
    }

    #[test]
    fn sensitive_pattern_rejects_false_positives() {
        assert!(!matches_sensitive_pattern("/home/u/project/src/main.rs"));
        assert!(!matches_sensitive_pattern("/home/u/.claude/projects/data.jsonl"));
        assert!(!matches_sensitive_pattern("/home/u/documents/env_notes.txt"));
    }
}
