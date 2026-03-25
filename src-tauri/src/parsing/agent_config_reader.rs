//! Agent Config Reader
//!
//! Reads `.claude/agents/*.md` files from a project directory and extracts
//! frontmatter metadata (name, color) for use in subagent visualization.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Agent configuration extracted from .md file frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AgentConfig {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Parse simple YAML frontmatter from markdown content.
/// Only extracts top-level scalar key: value pairs between --- delimiters.
fn parse_frontmatter(content: &str) -> HashMap<String, String> {
    let content = content.trim_start();

    // Check for frontmatter delimiter
    if !content.starts_with("---\n") && !content.starts_with("---\r\n") {
        return HashMap::new();
    }

    // Find the closing delimiter
    let after_first_delim = &content[3..]; // Skip first ---
    let newline_pos = after_first_delim.find('\n').unwrap_or(0);
    let after_first_newline = &after_first_delim[newline_pos + 1..];

    let end_pos = after_first_newline.find("\n---").or_else(|| after_first_newline.find("\r\n---"));
    let frontmatter_content = match end_pos {
        Some(pos) => &after_first_newline[..pos],
        None => return HashMap::new(),
    };

    let mut result = HashMap::new();

    for line in frontmatter_content.lines() {
        let colon_idx = match line.find(':') {
            Some(idx) => idx,
            None => continue,
        };

        let key = line[..colon_idx].trim();
        let mut value = line[colon_idx + 1..].trim();

        // Strip surrounding quotes
        if (value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\''))
        {
            value = &value[1..value.len() - 1];
        }

        if !key.is_empty() {
            result.insert(key.to_string(), value.to_string());
        }
    }

    result
}

/// Read agent config files from a project's `.claude/agents/` directory.
/// Returns a map of agent name → config (with optional color).
pub fn read_agent_configs(project_root: &str) -> HashMap<String, AgentConfig> {
    let agents_dir = Path::new(project_root).join(".claude").join("agents");
    let mut result = HashMap::new();

    let entries = match fs::read_dir(&agents_dir) {
        Ok(entries) => entries,
        Err(_) => return result, // Directory doesn't exist — normal for projects without custom agents
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if !filename.ends_with(".md") {
            continue;
        }

        if let Ok(content) = fs::read_to_string(&path) {
            let frontmatter = parse_frontmatter(&content);
            let name = frontmatter
                .get("name")
                .cloned()
                .unwrap_or_else(|| filename.trim_end_matches(".md").to_string());

            let config = AgentConfig {
                name: name.clone(),
                color: frontmatter.get("color").cloned(),
            };

            result.insert(name, config);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_parse_frontmatter_simple() {
        let content = r#"---
name: test-agent
color: blue
---
# Agent content
"#;
        let result = parse_frontmatter(content);
        assert_eq!(result.get("name"), Some(&"test-agent".to_string()));
        assert_eq!(result.get("color"), Some(&"blue".to_string()));
    }

    #[test]
    fn test_parse_frontmatter_with_quotes() {
        let content = r#"---
name: "quoted name"
color: 'red'
---
Content
"#;
        let result = parse_frontmatter(content);
        assert_eq!(result.get("name"), Some(&"quoted name".to_string()));
        assert_eq!(result.get("color"), Some(&"red".to_string()));
    }

    #[test]
    fn test_parse_frontmatter_no_delimiter() {
        let content = "# No frontmatter\nContent";
        let result = parse_frontmatter(content);
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_frontmatter_empty() {
        let content = "";
        let result = parse_frontmatter(content);
        assert!(result.is_empty());
    }

    #[test]
    fn test_read_agent_configs() {
        let temp_dir = TempDir::new().unwrap();
        let agents_dir = temp_dir.path().join(".claude").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        // Create agent files
        fs::write(
            agents_dir.join("coder.md"),
            "---\nname: coder\ncolor: green\n---\n# Coder agent",
        )
        .unwrap();
        fs::write(
            agents_dir.join("reviewer.md"),
            "---\nname: reviewer\n---\n# Reviewer agent",
        )
        .unwrap();
        fs::write(agents_dir.join("plain.md"), "# Plain agent").unwrap();

        let result = read_agent_configs(&temp_dir.path().to_string_lossy());

        assert_eq!(result.len(), 3);
        assert_eq!(result.get("coder").unwrap().color, Some("green".to_string()));
        assert_eq!(result.get("reviewer").unwrap().color, None);
        assert_eq!(result.get("plain").unwrap().name, "plain");
    }

    #[test]
    fn test_read_agent_configs_no_dir() {
        let temp_dir = TempDir::new().unwrap();
        let result = read_agent_configs(&temp_dir.path().to_string_lossy());
        assert!(result.is_empty());
    }

    #[test]
    fn test_read_agent_configs_uses_filename_as_default_name() {
        let temp_dir = TempDir::new().unwrap();
        let agents_dir = temp_dir.path().join(".claude").join("agents");
        fs::create_dir_all(&agents_dir).unwrap();

        fs::write(agents_dir.join("my-agent.md"), "# No frontmatter").unwrap();

        let result = read_agent_configs(&temp_dir.path().to_string_lossy());

        assert_eq!(result.len(), 1);
        assert_eq!(result.get("my-agent").unwrap().name, "my-agent");
    }
}