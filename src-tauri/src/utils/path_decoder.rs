use std::path::PathBuf;

/// Encode absolute path to Claude Code directory name. Lossy for paths with dashes.
/// "/Users/name/project" -> "-Users-name-project"
pub fn encode_path(absolute_path: &str) -> String {
    if absolute_path.is_empty() {
        return String::new();
    }
    let encoded: String = absolute_path.chars().map(|c| if c == '/' || c == '\\' { '-' } else { c }).collect();
    if encoded.starts_with('-') { encoded } else { format!("-{encoded}") }
}

/// Decode directory name back to path (best-effort; lossy for dashes).
pub fn decode_path(encoded_name: &str) -> String {
    if encoded_name.is_empty() { return String::new(); }
    if let Some(rest) = legacy_windows_drive(encoded_name) { return rest; }
    let without_leading = encoded_name.strip_prefix('-').unwrap_or(encoded_name);
    let decoded: String = without_leading.chars().map(|c| if c == '-' { '/' } else { c }).collect();
    if looks_like_windows_drive(&decoded) { return translate_wsl_mount(&decoded); }
    let absolute = if decoded.starts_with('/') { decoded } else { format!("/{decoded}") };
    translate_wsl_mount(&absolute)
}

/// Extract project name (last path segment). Prefers `cwd_hint` to avoid lossy decode.
pub fn extract_project_name(encoded_name: &str, cwd_hint: Option<&str>) -> String {
    if let Some(hint) = cwd_hint {
        if let Some(name) = hint.split(&['/', '\\']).filter(|s| !s.is_empty()).next_back() {
            return name.to_string();
        }
    }
    let decoded = decode_path(encoded_name);
    decoded.split('/').filter(|s| !s.is_empty()).next_back().unwrap_or(encoded_name).to_string()
}

/// Validate encoded path format (POSIX, Windows drive, or legacy Windows).
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

/// Validate project ID (plain encoded path or composite `{encoded}::{8-hex}`).
pub fn is_valid_project_id(project_id: &str) -> bool {
    if project_id.is_empty() { return false; }
    match project_id.find("::") {
        None => is_valid_encoded_path(project_id),
        Some(sep) => is_valid_encoded_path(&project_id[..sep]) && is_hex8(&project_id[sep + 2..]),
    }
}

/// Extract base directory from project ID (strips composite `::{hash}` suffix).
pub fn extract_base_dir(project_id: &str) -> &str {
    match project_id.find("::") {
        Some(sep) => &project_id[..sep],
        None => project_id,
    }
}

/// Extract session ID from filename, stripping `.jsonl` extension.
pub fn extract_session_id(filename: &str) -> String {
    filename.strip_suffix(".jsonl").unwrap_or(filename).to_string()
}

/// Build session file path: `{claude_base}/projects/{project_id}/{session_id}.jsonl`.
pub fn build_session_path(claude_base: &str, project_id: &str, session_id: &str) -> PathBuf {
    PathBuf::from(claude_base).join("projects").join(extract_base_dir(project_id)).join(format!("{session_id}.jsonl"))
}

/// Build subagents directory: `{claude_base}/projects/{project_id}/{session_id}/subagents`.
pub fn build_subagents_path(claude_base: &str, project_id: &str, session_id: &str) -> PathBuf {
    PathBuf::from(claude_base).join("projects").join(extract_base_dir(project_id)).join(session_id).join("subagents")
}

/// Build todo file path: `{claude_base}/todos/{session_id}.json`.
pub fn build_todo_path(claude_base: &str, session_id: &str) -> PathBuf {
    PathBuf::from(claude_base).join("todos").join(format!("{session_id}.json"))
}

/// Return default `~/.claude` path.
pub fn get_default_claude_base_path() -> PathBuf {
    dirs::home_dir().map(|h| h.join(".claude")).unwrap_or_else(|| PathBuf::from("/.claude"))
}

/// Return `~/.claude/projects`.
pub fn get_projects_base_path() -> PathBuf { get_default_claude_base_path().join("projects") }

/// Return `~/.claude/todos`.
pub fn get_todos_base_path() -> PathBuf { get_default_claude_base_path().join("todos") }

/// Check if path is a subagent file (contains `/subagents/`).
pub fn is_subagent_file(path: &str) -> bool { path.contains("/subagents/") }

/// Extract project ID from a relative path within `~/.claude/projects/`.
pub fn extract_project_id_from_path(relative_path: &str) -> Option<String> {
    let stripped = relative_path.strip_prefix("projects/")?;
    Some(stripped[..stripped.find('/')?].to_string())
}

fn legacy_windows_drive(encoded_name: &str) -> Option<String> {
    let b = encoded_name.as_bytes();
    if b.len() < 4 || !b[0].is_ascii_alphabetic() || b[1] != b'-' || b[2] != b'-' { return None; }
    let rest: String = b[3..].iter().map(|&c| if c == b'-' { '/' } else { c as char }).collect::<String>();
    Some(format!("{}:/{rest}", (b[0] as char).to_ascii_uppercase()))
}

fn looks_like_windows_drive(d: &str) -> bool {
    let b = d.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/'
}

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

fn is_drive_colon_at_start(s: &str, pos: usize) -> bool {
    let b = s.as_bytes();
    s.len() >= 3 && b[0] == b'-' && b[1].is_ascii_alphabetic() && pos == 2 && b[2] == b':'
}

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
