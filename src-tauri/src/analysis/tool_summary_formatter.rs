//! Tool summary formatting for token threshold messages.
//!
//! Ported from Electron ToolSummaryFormatter.ts.

/// Format a token count for display.
/// Below 1000: raw number. 1k-10k: one decimal place. Above 10k: whole number.
pub fn format_tokens(tokens: usize) -> String {
    if tokens < 1000 {
        return tokens.to_string();
    }
    if tokens < 10_000 {
        return format!("{:.1}k", tokens as f64 / 1000.0);
    }
    format!("{}k", (tokens as f64 / 1000.0).round() as usize)
}

/// Generate a human-readable summary for a tool invocation.
pub fn get_tool_summary(tool_name: &str, input: &serde_json::Value) -> String {
    match tool_name {
        "Edit" | "Read" | "Write" => {
            extract_field(input, "file_path")
                .map(|p| basename(&p))
                .unwrap_or_else(|| tool_name.to_string())
        }
        "Bash" => {
            extract_field(input, "description")
                .map(|d| truncate(&d, 50))
                .or_else(|| extract_field(input, "command").map(|c| truncate(&c, 50)))
                .unwrap_or_else(|| "Bash".to_string())
        }
        "Grep" | "Glob" => {
            extract_field(input, "pattern")
                .map(|p| truncate(&p, 30))
                .unwrap_or_else(|| tool_name.to_string())
        }
        "Task" => {
            let sub = extract_field(input, "subagent_type").unwrap_or_default();
            let desc = extract_field(input, "description")
                .or_else(|| extract_field(input, "prompt"))
                .map(|d| truncate(&d, 40))
                .unwrap_or_default();
            if !sub.is_empty() && !desc.is_empty() {
                truncate(&format!("{} - {}", sub, desc), 40)
            } else if !sub.is_empty() {
                sub
            } else {
                "Task".to_string()
            }
        }
        "Skill" => {
            extract_field(input, "skill").unwrap_or_else(|| "Skill".to_string())
        }
        "WebFetch" => {
            extract_field(input, "url")
                .map(|u| {
                    let display = if u.contains("://") {
                        u.split("://").nth(1).unwrap_or(&u)
                    } else {
                        &u
                    };
                    truncate(display, 50)
                })
                .unwrap_or_else(|| "WebFetch".to_string())
        }
        "WebSearch" => {
            extract_field(input, "query")
                .map(|q| truncate(&q, 40))
                .unwrap_or_else(|| "WebSearch".to_string())
        }
        _ => {
            extract_field(input, "name")
                .or_else(|| extract_field(input, "path"))
                .or_else(|| extract_field(input, "file"))
                .or_else(|| extract_field(input, "query"))
                .or_else(|| extract_field(input, "command"))
                .map(|f| truncate(&f, 50))
                .unwrap_or_else(|| tool_name.to_string())
        }
    }
}

fn extract_field(input: &serde_json::Value, field: &str) -> Option<String> {
    input.get(field)?.as_str().map(|s| s.to_string())
}

fn basename(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    format!("{}...", &s[..max_len.saturating_sub(3)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_format_tokens_small() {
        assert_eq!(format_tokens(500), "500");
    }

    #[test]
    fn test_format_tokens_thousands() {
        assert_eq!(format_tokens(1500), "1.5k");
    }

    #[test]
    fn test_format_tokens_tens_of_thousands() {
        assert_eq!(format_tokens(25000), "25k");
    }

    #[test]
    fn test_get_tool_summary_bash() {
        let input = json!({"command": "npm install", "description": "Install dependencies"});
        assert_eq!(get_tool_summary("Bash", &input), "Install dependencies");
    }

    #[test]
    fn test_get_tool_summary_edit() {
        let input = json!({"file_path": "/Users/test/src/main.rs"});
        assert_eq!(get_tool_summary("Edit", &input), "main.rs");
    }

    #[test]
    fn test_get_tool_summary_unknown() {
        let input = json!({"name": "my-feature"});
        assert_eq!(get_tool_summary("CustomTool", &input), "my-feature");
    }

    #[test]
    fn test_truncate_short() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_long() {
        let result = truncate("a very long string that needs truncation", 20);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 20);
    }
}
