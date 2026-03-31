//! Session Content Filter for noise session detection.
//!
//! Determines whether a session JSONL file contains at least one "displayable"
//! (non-noise) entry. Sessions with only noise entries are filtered out from
//! the session listing.

use std::path::Path;

use crate::infrastructure::fs_provider::FsProvider;

/// Hard-coded noise message types that are never displayable.
const HARD_NOISE_TYPES: &[&str] = &[
    "system",
    "summary",
    "file-history-snapshot",
    "queue-operation",
];

/// Tags that indicate a user message is noise (system-generated).
const HARD_NOISE_TAGS: &[&str] = &[
    "<local-command-caveat>",
    "<system-reminder>",
];

/// Check if a JSONL session file contains at least one non-noise (displayable) entry.
///
/// Streams through the file and returns `true` on the first displayable entry found.
/// Returns `false` if all entries are noise, or if the file cannot be read.
pub fn has_non_noise_messages(path: &Path, fs_provider: &dyn FsProvider) -> bool {
    let content = match fs_provider.read_file_head(path, 200) {
        Ok(c) => c,
        Err(_) => return false,
    };

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let json: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Extract fields
        let msg_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let is_sidechain = json.get("isSidechain").and_then(|v| v.as_bool()).unwrap_or(false);
        let model = json.get("model").and_then(|v| v.as_str());

        // Skip hard noise types
        if HARD_NOISE_TYPES.contains(&msg_type) {
            continue;
        }

        // Skip sidechain entries
        if is_sidechain {
            continue;
        }

        // Check assistant entries
        if msg_type == "assistant" {
            // Skip synthetic assistants
            if model == Some("<synthetic>") {
                continue;
            }
            // Real assistant -> displayable
            return true;
        }

        // Check user entries
        if msg_type == "user" {
            // isMeta entries are displayable
            if json.get("isMeta").and_then(|v| v.as_bool()) == Some(true) {
                return true;
            }

            let content = json.get("content");

            // String content
            if let Some(text) = content.and_then(|c| c.as_str()) {
                // Check if wrapped in noise tags
                if is_wrapped_in_noise_tags(text) {
                    continue;
                }
                // Command output detection
                if text.starts_with("<local-command-stdout>")
                    || text.starts_with("<local-command-stderr>")
                {
                    return true;
                }
                // Non-empty string -> displayable
                if !text.trim().is_empty() {
                    return true;
                }
                continue;
            }

            // Array content
            if let Some(blocks) = content.and_then(|c| c.as_array()) {
                // Has tool_result block -> displayable
                if blocks.iter().any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result")) {
                    return true;
                }
                // Has text/image blocks -> check noise tags per block
                for block in blocks {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        if !is_wrapped_in_noise_tags(text) && !text.trim().is_empty() {
                            return true;
                        }
                    }
                    // Image blocks are displayable
                    if block.get("type").and_then(|t| t.as_str()) == Some("image") {
                        return true;
                    }
                }
            }
        }
    }

    false
}

/// Check if text is entirely wrapped in a hard noise tag.
fn is_wrapped_in_noise_tags(text: &str) -> bool {
    let trimmed = text.trim();
    HARD_NOISE_TAGS.iter().any(|tag| {
        trimmed.starts_with(tag)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn provider() -> crate::infrastructure::fs_provider::LocalFsProvider {
        crate::infrastructure::fs_provider::LocalFsProvider::new()
    }

    #[test]
    fn test_all_noise_returns_false() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("noise.jsonl");
        let lines = [
            r#"{"type":"system","message":"init"}"#,
            r#"{"type":"summary","summary":"..."}"#,
            r#"{"type":"assistant","model":"<synthetic>","message":"auto"}"#,
            r#"{"type":"user","content":"<system-reminder>check this</system-reminder>","isSidechain":false}"#,
        ];
        fs::write(&path, lines.join("\n")).unwrap();

        assert!(!has_non_noise_messages(&path, &provider()));
    }

    #[test]
    fn test_real_assistant_returns_true() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("real.jsonl");
        let lines = [
            r#"{"type":"system","message":"init"}"#,
            r#"{"type":"assistant","model":"claude-3-opus","message":"hello"}"#,
        ];
        fs::write(&path, lines.join("\n")).unwrap();

        assert!(has_non_noise_messages(&path, &provider()));
    }

    #[test]
    fn test_real_user_returns_true() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("user.jsonl");
        let lines = [
            r#"{"type":"system","message":"init"}"#,
            r#"{"type":"user","content":"Hello, Claude!","isSidechain":false}"#,
        ];
        fs::write(&path, lines.join("\n")).unwrap();

        assert!(has_non_noise_messages(&path, &provider()));
    }

    #[test]
    fn test_is_meta_returns_true() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meta.jsonl");
        let lines = [
            r#"{"type":"user","content":"","isMeta":true,"isSidechain":false}"#,
        ];
        fs::write(&path, lines.join("\n")).unwrap();

        assert!(has_non_noise_messages(&path, &provider()));
    }

    #[test]
    fn test_tool_result_returns_true() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tool.jsonl");
        let lines = [
            r#"{"type":"user","content":[{"type":"tool_result","tool_use_id":"abc","content":"result"}],"isSidechain":false}"#,
        ];
        fs::write(&path, lines.join("\n")).unwrap();

        assert!(has_non_noise_messages(&path, &provider()));
    }

    #[test]
    fn test_command_output_returns_true() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cmd.jsonl");
        let lines = [
            r#"{"type":"user","content":"<local-command-stdout>build ok</local-command-stdout>","isSidechain":false}"#,
        ];
        fs::write(&path, lines.join("\n")).unwrap();

        assert!(has_non_noise_messages(&path, &provider()));
    }

    #[test]
    fn test_sidechain_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sidechain.jsonl");
        let lines = [
            r#"{"type":"assistant","model":"claude-3-opus","isSidechain":true,"message":"side"}"#,
        ];
        fs::write(&path, lines.join("\n")).unwrap();

        assert!(!has_non_noise_messages(&path, &provider()));
    }

    #[test]
    fn test_image_block_returns_true() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image.jsonl");
        let lines = [
            r#"{"type":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"abc"}}],"isSidechain":false}"#,
        ];
        fs::write(&path, lines.join("\n")).unwrap();

        assert!(has_non_noise_messages(&path, &provider()));
    }

    #[test]
    fn test_nonexistent_file_returns_false() {
        let path = std::path::PathBuf::from("/nonexistent/file.jsonl");
        assert!(!has_non_noise_messages(&path, &provider()));
    }

    #[test]
    fn test_local_command_caveat_noise() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("caveat.jsonl");
        let lines = [
            r#"{"type":"user","content":"<local-command-caveat>some system text</local-command-caveat>","isSidechain":false}"#,
        ];
        fs::write(&path, lines.join("\n")).unwrap();

        assert!(!has_non_noise_messages(&path, &provider()));
    }

    #[test]
    fn test_file_history_snapshot_noise() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fhs.jsonl");
        let lines = [
            r#"{"type":"file-history-snapshot","files":[]}"#,
        ];
        fs::write(&path, lines.join("\n")).unwrap();

        assert!(!has_non_noise_messages(&path, &provider()));
    }
}
