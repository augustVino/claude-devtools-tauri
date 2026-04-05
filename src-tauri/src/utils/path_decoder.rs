//! 路径编解码工具模块。
//!
//! 处理 Claude Code 项目目录名与绝对路径之间的编解码转换。
//! 编码规则：将路径分隔符 `/` 或 `\` 替换为 `-`，并在开头添加 `-` 前缀。
//! 注意：含 `-` 的路径在编解码过程中会有信息丢失。

use std::path::PathBuf;
use std::sync::RwLock;

/// Global override for the Claude root path.
/// Set once during app setup via `set_claude_root_override()`.
/// When `Some`, `get_projects_base_path()` and `get_todos_base_path()` use this instead of the default.
static CLAUDE_ROOT_OVERRIDE: RwLock<Option<String>> = RwLock::new(None);

/// Set the global Claude root path override. Called during app setup after config is loaded.
pub fn set_claude_root_override(path: Option<String>) {
    if let Ok(mut guard) = CLAUDE_ROOT_OVERRIDE.write() {
        *guard = path.filter(|p| !p.is_empty());
    }
}

/// 将绝对路径编码为 Claude Code 目录名。对含 `-` 的路径有损。
/// 示例：`"/Users/name/project"` -> `"-Users-name-project"`
pub fn encode_path(absolute_path: &str) -> String {
    if absolute_path.is_empty() {
        return String::new();
    }
    let encoded: String = absolute_path.chars().map(|c| if c == '/' || c == '\\' { '-' } else { c }).collect();
    if encoded.starts_with('-') { encoded } else { format!("-{encoded}") }
}

/// 将目录名解码回路径（尽力而为；对含 `-` 的路径有损）。
pub fn decode_path(encoded_name: &str) -> String {
    if encoded_name.is_empty() { return String::new(); }
    if let Some(rest) = legacy_windows_drive(encoded_name) { return rest; }
    let without_leading = encoded_name.strip_prefix('-').unwrap_or(encoded_name);
    let decoded: String = without_leading.chars().map(|c| if c == '-' { '/' } else { c }).collect();
    if looks_like_windows_drive(&decoded) { return translate_wsl_mount(&decoded); }
    let absolute = if decoded.starts_with('/') { decoded } else { format!("/{decoded}") };
    translate_wsl_mount(&absolute)
}

/// 提取项目名称（路径最后一段）。优先使用 `cwd_hint` 以避免有损解码。
pub fn extract_project_name(encoded_name: &str, cwd_hint: Option<&str>) -> String {
    if let Some(hint) = cwd_hint {
        if let Some(name) = hint.split(&['/', '\\']).filter(|s| !s.is_empty()).next_back() {
            return name.to_string();
        }
    }
    let decoded = decode_path(encoded_name);
    decoded.split('/').filter(|s| !s.is_empty()).next_back().unwrap_or(encoded_name).to_string()
}

/// 验证编码路径格式（POSIX、Windows 盘符或旧版 Windows 格式）。
pub fn is_valid_encoded_path(encoded_name: &str) -> bool {
    if encoded_name.is_empty() { return false; }
    let legacy_re = regex::Regex::new(r"^[a-zA-Z]--[a-zA-Z0-9_.\s-]+$").unwrap();
    if legacy_re.is_match(encoded_name) { return true; }
    if !encoded_name.starts_with('-') { return false; }
    let valid_re = regex::Regex::new(r"^-[a-zA-Z0-9_.\s:\-]+$").unwrap();
    if !valid_re.is_match(encoded_name) { return false; }
    if let Some(pos) = encoded_name.find(':') {
        if !is_drive_colon_at_start(encoded_name, pos) || encoded_name[pos + 1..].contains(':') {
            return false;
        }
    }
    true
}

/// 验证项目 ID 格式（纯编码路径或复合格式 `{编码路径}::{8位十六进制}`）。
pub fn is_valid_project_id(project_id: &str) -> bool {
    if project_id.is_empty() { return false; }
    match project_id.find("::") {
        None => is_valid_encoded_path(project_id),
        Some(sep) => is_valid_encoded_path(&project_id[..sep]) && is_hex8(&project_id[sep + 2..]),
    }
}

/// 从项目 ID 中提取基础目录（去除复合格式的 `::{hash}` 后缀）。
pub fn extract_base_dir(project_id: &str) -> &str {
    match project_id.find("::") {
        Some(sep) => &project_id[..sep],
        None => project_id,
    }
}

/// 从文件名中提取会话 ID，去除 `.jsonl` 扩展名。
pub fn extract_session_id(filename: &str) -> String {
    filename.strip_suffix(".jsonl").unwrap_or(filename).to_string()
}

/// 构建会话文件路径：`{claude_base}/projects/{project_id}/{session_id}.jsonl`。
#[allow(dead_code)]
pub fn build_session_path(claude_base: &str, project_id: &str, session_id: &str) -> PathBuf {
    PathBuf::from(claude_base).join("projects").join(extract_base_dir(project_id)).join(format!("{session_id}.jsonl"))
}

/// 构建子 Agent 目录路径：`{claude_base}/projects/{project_id}/{session_id}/subagents`。
#[allow(dead_code)]
pub fn build_subagents_path(claude_base: &str, project_id: &str, session_id: &str) -> PathBuf {
    PathBuf::from(claude_base).join("projects").join(extract_base_dir(project_id)).join(session_id).join("subagents")
}

/// 构建待办事项文件路径：`{claude_base}/todos/{session_id}.json`。
#[allow(dead_code)]
pub fn build_todo_path(claude_base: &str, session_id: &str) -> PathBuf {
    PathBuf::from(claude_base).join("todos").join(format!("{session_id}.json"))
}

/// 返回默认的 `~/.claude` 路径。
pub fn get_default_claude_base_path() -> PathBuf {
    dirs::home_dir().map(|h| h.join(".claude")).unwrap_or_else(|| PathBuf::from("/.claude"))
}

/// 根据配置返回有效的 Claude base path。
/// 如果 `claude_root_path` 为 `Some` 且非空，使用自定义路径；否则使用默认 `~/.claude`。
fn resolve_claude_base_path(claude_root_path: Option<&str>) -> PathBuf {
    match claude_root_path {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => get_default_claude_base_path(),
    }
}

/// 返回有效的 projects 路径。
/// 如果设置了 claude_root_path override，使用自定义路径；否则使用默认 `~/.claude/projects`。
pub fn get_projects_base_path() -> PathBuf {
    let claude_root = CLAUDE_ROOT_OVERRIDE.read().ok().and_then(|g| g.clone());
    resolve_claude_base_path(claude_root.as_deref()).join("projects")
}

/// 返回有效的 todos 路径。
/// 如果设置了 claude_root_path override，使用自定义路径；否则使用默认 `~/.claude/todos`。
pub fn get_todos_base_path() -> PathBuf {
    let claude_root = CLAUDE_ROOT_OVERRIDE.read().ok().and_then(|g| g.clone());
    resolve_claude_base_path(claude_root.as_deref()).join("todos")
}

/// 检查路径是否为子 Agent 文件（路径包含 `/subagents/`）。
pub fn is_subagent_file(path: &str) -> bool { path.contains("/subagents/") }

/// 从 `~/.claude/projects/` 下的相对路径中提取项目 ID。
#[allow(dead_code)]
pub fn extract_project_id_from_path(relative_path: &str) -> Option<String> {
    let stripped = relative_path.strip_prefix("projects/")?;
    Some(stripped[..stripped.find('/')?].to_string())
}

/// 处理旧版 Windows 盘符编码格式（如 `C--Users-name` -> `C:/Users/name`）。
fn legacy_windows_drive(encoded_name: &str) -> Option<String> {
    let b = encoded_name.as_bytes();
    if b.len() < 4 || !b[0].is_ascii_alphabetic() || b[1] != b'-' || b[2] != b'-' { return None; }
    let rest: String = b[3..].iter().map(|&c| if c == b'-' { '/' } else { c as char }).collect::<String>();
    Some(format!("{}:/{rest}", (b[0] as char).to_ascii_uppercase()))
}

/// 检查路径是否形如 Windows 盘符格式（如 `C:/`）。
fn looks_like_windows_drive(d: &str) -> bool {
    let b = d.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/'
}

/// 在 Windows 平台上将 WSL 挂载路径（如 `/mnt/c/`）转换为 Windows 路径（如 `C:/`）。
fn translate_wsl_mount(posix_path: &str) -> String {
    if cfg!(target_os = "windows") {
        if let Some(rest) = posix_path.strip_prefix("/mnt/") {
            if let Some(drive) = rest.chars().next().filter(|c| c.is_ascii_alphabetic()) {
                let rem = &rest[drive.len_utf8()..];
                let sep = if rem.is_empty() || rem.starts_with('/') { "" } else { "/" };
                return format!("{}:{}{}", drive.to_ascii_uppercase(), sep, rem);
            }
        }
    }
    posix_path.to_string()
}

/// 检查冒号是否出现在盘符位置（格式 `-X:`，其中 X 为字母）。
fn is_drive_colon_at_start(s: &str, pos: usize) -> bool {
    let b = s.as_bytes();
    s.len() >= 3 && b[0] == b'-' && b[1].is_ascii_alphabetic() && pos == 2 && b[2] == b':'
}

/// 检查字符串是否为 8 位十六进制值（项目 ID 后缀格式）。
fn is_hex8(s: &str) -> bool { s.len() == 8 && s.chars().all(|c| c.is_ascii_hexdigit()) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_round_trip() {
        let p = "/Users/name/project";
        assert_eq!(encode_path(p), "-Users-name-project");
        assert_eq!(decode_path("-Users-name-project"), p);
        assert_eq!(decode_path(&encode_path(p)), p);
    }

    #[test]
    fn encode_windows_and_edge_cases() {
        assert_eq!(encode_path("C:\\Users\\name\\project"), "-C:-Users-name-project");
        assert_eq!(encode_path(""), "");
        assert_eq!(encode_path("-foo"), "-foo");
        assert_eq!(decode_path(""), "");
    }

    #[test]
    fn round_trip_lossy() {
        assert_ne!(decode_path(&encode_path("/home/my-project")), "/home/my-project");
    }

    #[test]
    fn test_extract_session_id() {
        assert_eq!(extract_session_id("abc123.jsonl"), "abc123");
        assert_eq!(extract_session_id("abc123"), "abc123");
        assert_eq!(extract_session_id(""), "");
    }

    #[test]
    fn valid_encoded_paths() {
        assert!(is_valid_encoded_path("-Users-name-project"));
        assert!(is_valid_encoded_path("-Users-name.my-project"));
        assert!(is_valid_encoded_path("-C:-Users-name-project"));
        assert!(is_valid_encoded_path("C--Users-name-project"));
    }

    #[test]
    fn invalid_encoded_paths() {
        assert!(!is_valid_encoded_path(""));
        assert!(!is_valid_encoded_path("Users-name-project"));
        assert!(!is_valid_encoded_path("-Users-name:project"));
    }

    #[test]
    fn project_id_validation() {
        assert!(is_valid_project_id("-Users-name-project"));
        assert!(is_valid_project_id("-Users-name-project::abcd1234"));
        assert!(!is_valid_project_id("-Users-name-project::xyz"));
        assert!(!is_valid_project_id(""));
    }

    #[test]
    fn extract_base_dir_and_name() {
        assert_eq!(extract_base_dir("-Users-name-project"), "-Users-name-project");
        assert_eq!(extract_base_dir("-Users-name-project::abcd1234"), "-Users-name-project");
        assert_eq!(extract_project_name("-Users-name-project", None), "project");
        assert_eq!(
            extract_project_name("-Users-claude-devtools", Some("/home/user/claude-devtools")),
            "claude-devtools"
        );
    }

    #[test]
    fn build_paths() {
        assert_eq!(build_session_path("/h/.claude", "-U-n", "s1"), PathBuf::from("/h/.claude/projects/-U-n/s1.jsonl"));
        assert_eq!(build_session_path("/h/.claude", "-U-n::abcd1234", "s1"), PathBuf::from("/h/.claude/projects/-U-n/s1.jsonl"));
        assert_eq!(build_subagents_path("/h/.claude", "-U-n", "s1"), PathBuf::from("/h/.claude/projects/-U-n/s1/subagents"));
        assert_eq!(build_todo_path("/h/.claude", "s1"), PathBuf::from("/h/.claude/todos/s1.json"));
    }

    #[test]
    fn path_helpers() {
        assert!(is_subagent_file("/p/X/s/subagents/child.jsonl"));
        assert!(!is_subagent_file("/p/X/s.jsonl"));
        assert_eq!(extract_project_id_from_path("projects/-U-n/s.jsonl"), Some("-U-n".to_string()));
        assert_eq!(extract_project_id_from_path("-U-n"), None);
    }

    #[test]
    fn decode_legacy_windows() {
        assert_eq!(decode_path("C--Users-name-project"), "C:/Users/name/project");
    }

    #[test]
    fn base_paths() {
        assert!(get_default_claude_base_path().to_string_lossy().ends_with(".claude"));
        assert!(get_projects_base_path().to_string_lossy().ends_with("projects"));
        assert!(get_todos_base_path().to_string_lossy().ends_with("todos"));
    }
}
