//! Memory viewer types and MEMORY.md index parser.

use serde::{Deserialize, Serialize};

/// Parsed entry from MEMORY.md index file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntry {
    pub title: String,
    pub file: String,
    pub hook: String,
    pub line_number: usize,
}

/// Parsed MEMORY.md index.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryIndex {
    pub raw_markdown: String,
    pub entries: Vec<MemoryEntry>,
    pub orphan_files: Vec<String>,
}

/// A single memory file's content and metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryFile {
    pub file_name: String,
    pub absolute_path: String,
    pub content: String,
}

/// Result for `read_memory_file` command — flat struct with `success: bool`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryReadFileResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Result for `copy_memory_path` command — flat struct with `success: bool`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryOpenResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 可打开的外部应用目标。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenTarget {
    pub id: String,
    pub label: String,
    pub icon_name: String,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shortcut_key: Option<String>,
}

/// Parse MEMORY.md content and a directory listing into a `MemoryIndex`.
///
/// Entry regex: `- [Title](file.md) — hook`
/// Bounded character classes for linear-time matching (no backtracking).
static MEMORY_INDEX_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^\s*-\s*\[([^\]\n]+)\]\(([^)\n]+\.md)\)\s*(?:[—–-]\s*(.*))?$")
        .expect("memory index regex is valid")
});

pub fn parse_memory_index(markdown: &str, dir_listing: &[String]) -> MemoryIndex {
    let mut entries = Vec::new();
    let mut seen_files = std::collections::HashSet::new();

    for (idx, line) in markdown.lines().enumerate() {
        let Some(caps) = MEMORY_INDEX_RE.captures(line) else {
            continue;
        };
        let title = caps[1].trim().to_string();
        let file = caps[2].trim().to_string();
        let hook = caps.get(3).map_or("", |m| m.as_str()).trim().to_string();
        if title.is_empty() || file.is_empty() {
            continue;
        }
        seen_files.insert(file.clone());
        entries.push(MemoryEntry {
            title,
            file,
            hook,
            line_number: idx + 1,
        });
    }

    let mut orphan_files: Vec<String> = dir_listing
        .iter()
        .filter(|name| {
            let lower = name.to_lowercase();
            lower.ends_with(".md") && name.as_str() != "MEMORY.md" && !seen_files.contains(*name)
        })
        .cloned()
        .collect();
    orphan_files.sort();

    MemoryIndex {
        raw_markdown: markdown.to_string(),
        entries,
        orphan_files,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_markdown() {
        let result = parse_memory_index("", &[]);
        assert!(result.entries.is_empty());
        assert!(result.orphan_files.is_empty());
    }

    #[test]
    fn parse_entries() {
        let md = "- [User Role](user-role.md) — user profile info\n- [Feedback](feedback.md) — correction guidance";
        let dir = vec!["user-role.md".into(), "feedback.md".into(), "MEMORY.md".into()];
        let result = parse_memory_index(md, &dir);
        assert_eq!(result.entries.len(), 2);
        assert_eq!(result.entries[0].title, "User Role");
        assert_eq!(result.entries[0].file, "user-role.md");
        assert_eq!(result.entries[0].hook, "user profile info");
        assert_eq!(result.entries[0].line_number, 1);
        assert_eq!(result.entries[1].title, "Feedback");
        assert!(result.orphan_files.is_empty());
    }

    #[test]
    fn detect_orphan_files() {
        let md = "- [Linked](linked.md) — some hook\n";
        let dir = vec![
            "linked.md".into(),
            "unlinked.md".into(),
            "MEMORY.md".into(),
            "another-orphan.md".into(),
        ];
        let result = parse_memory_index(md, &dir);
        assert_eq!(result.orphan_files, vec!["another-orphan.md", "unlinked.md"]);
    }

    #[test]
    fn non_matching_lines_ignored() {
        let md = "# Header\n\nSome text\n- [Good](good.md) — ok\nNo match here";
        let result = parse_memory_index(md, &["good.md".into()]);
        assert_eq!(result.entries.len(), 1);
    }

    #[test]
    fn serialize_uses_camel_case() {
        let entry = MemoryEntry {
            title: "Test".into(),
            file: "test.md".into(),
            hook: "hook".into(),
            line_number: 1,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("lineNumber"), "Expected camelCase: {json}");
        assert!(!json.contains("line_number"), "Expected camelCase: {json}");
    }

    #[test]
    fn read_file_result_success_serializes_correctly() {
        let result = MemoryReadFileResult {
            success: true,
            content: Some("hello".into()),
            path: Some("/tmp/test.md".into()),
            error: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"success\":true"), "Expected boolean true: {json}");
        assert!(!json.contains("\"true\""), "Should not be string: {json}");
    }

    #[test]
    fn open_target_serializes_camel_case() {
        let target = OpenTarget {
            id: "vscode".into(),
            label: "VS Code".into(),
            icon_name: "file-code".into(),
            available: true,
            shortcut_key: Some("\u{2318}O".into()),
        };
        let json = serde_json::to_string(&target).unwrap();
        assert!(json.contains("\"iconName\""), "Expected camelCase: {json}");
        assert!(json.contains("\"shortcutKey\""), "Expected camelCase: {json}");
        assert!(!json.contains("icon_name"), "No snake_case: {json}");
    }

    #[test]
    fn open_target_none_shortcut_skipped() {
        let target = OpenTarget {
            id: "finder".into(),
            label: "Finder".into(),
            icon_name: "folder".into(),
            available: true,
            shortcut_key: None,
        };
        let json = serde_json::to_string(&target).unwrap();
        assert!(
            !json.contains("shortcutKey"),
            "shortcutKey should be omitted when None: {json}"
        );
    }
}
