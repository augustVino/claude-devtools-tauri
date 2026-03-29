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
        // --- Enhanced: separate Edit/Read/Write ---
        "Edit" => {
            let file = extract_field(input, "file_path").map(|f| basename(&f)).unwrap_or_else(|| "Edit".to_string());
            let old_lines = input.get("old_string").and_then(|v| v.as_str()).map(|s| s.lines().count());
            let new_lines = input.get("new_string").and_then(|v| v.as_str()).map(|s| s.lines().count());
            match (old_lines, new_lines) {
                (Some(o), Some(n)) if o != n => format!("{} - {} -> {} lines", file, o, n),
                (Some(n), _) => format!("{} - {} lines", file, n),
                _ => file,
            }
        }
        "Read" => {
            let file = extract_field(input, "file_path").map(|f| basename(&f)).unwrap_or_else(|| "Read".to_string());
            match (input.get("limit").and_then(|v| v.as_u64()), input.get("offset").and_then(|v| v.as_u64())) {
                (Some(limit), Some(offset)) => format!("{} - lines {}-{}", file, offset + 1, offset + limit),
                (Some(limit), None) => format!("{} - lines 1-{}", file, limit),
                _ => file,
            }
        }
        "Write" => {
            let file = extract_field(input, "file_path").map(|f| basename(&f)).unwrap_or_else(|| "Write".to_string());
            input.get("content").and_then(|v| v.as_str()).map(|s| {
                let lines = s.lines().count();
                format!("{} - {} lines", file, lines)
            }).unwrap_or(file)
        }
        // --- Existing: unchanged ---
        "Bash" => {
            extract_field(input, "description")
                .map(|d| truncate(&d, 50))
                .or_else(|| extract_field(input, "command").map(|c| truncate(&c, 50)))
                .unwrap_or_else(|| "Bash".to_string())
        }
        // --- Enhanced: separate Grep/Glob ---
        "Grep" => {
            let pattern = extract_field(input, "pattern").map(|p| truncate(&p, 30)).unwrap_or_else(|| "Grep".to_string());
            let scope = extract_field(input, "glob")
                .or_else(|| extract_field(input, "path").map(|p| basename(&p)));
            match scope {
                Some(s) => format!(r#""{}" in {}"#, pattern, s),
                None => format!(r#""{}""#, pattern),
            }
        }
        "Glob" => {
            let pattern = extract_field(input, "pattern").map(|p| truncate(&p, 30)).unwrap_or_else(|| "Glob".to_string());
            match extract_field(input, "path").map(|p| basename(&p)) {
                Some(s) => format!(r#""{}" in {}"#, pattern, s),
                None => format!(r#""{}""#, pattern),
            }
        }
        // --- Existing: unchanged ---
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
        // --- Enhanced: WebFetch with url::Url ---
        "WebFetch" => {
            extract_field(input, "url")
                .map(|u| {
                    match url::Url::parse(&u) {
                        Ok(parsed) => {
                            let display = format!("{}{}", parsed.host_str().unwrap_or(""), parsed.path());
                            truncate(&display, 50)
                        }
                        Err(_) => truncate(&u, 50),
                    }
                })
                .unwrap_or_else(|| "WebFetch".to_string())
        }
        // --- Enhanced: WebSearch with quotes ---
        "WebSearch" => {
            extract_field(input, "query")
                .map(|q| truncate(&format!(r#""{}""#, q), 42))
                .unwrap_or_else(|| "WebSearch".to_string())
        }
        // --- NEW: Team tools ---
        "TeamCreate" => {
            let name = extract_field(input, "team_name").unwrap_or_else(|| "Create team".to_string());
            match extract_field(input, "description").map(|d| truncate(&d, 30)) {
                Some(desc) => format!("{} - {}", name, desc),
                None => name,
            }
        }
        "TaskCreate" => {
            extract_field(input, "subject")
                .map(|s| truncate(&s, 50))
                .unwrap_or_else(|| "Create task".to_string())
        }
        "TaskUpdate" => {
            let mut parts = Vec::new();
            if let Some(tid) = extract_field(input, "taskId") {
                parts.push(format!("#{}", tid));
            }
            if let Some(status) = extract_field(input, "status") {
                parts.push(status);
            }
            if let Some(owner) = extract_field(input, "owner") {
                parts.push(format!("-> {}", owner));
            }
            if parts.is_empty() {
                "Update task".to_string()
            } else {
                parts.join(" ")
            }
        }
        "TaskList" => "List tasks".to_string(),
        "TaskGet" => {
            extract_field(input, "taskId")
                .map(|tid| format!("Get task #{}", tid))
                .unwrap_or_else(|| "Get task".to_string())
        }
        "SendMessage" => {
            let msg_type = extract_field(input, "type");
            let recipient = extract_field(input, "recipient");
            let summary = extract_field(input, "summary").map(|s| truncate(&s, 30));
            match msg_type.as_deref() {
                Some("shutdown_request") => {
                    recipient.map(|r| format!("Shutdown {}", r)).unwrap_or_else(|| "Shutdown".to_string())
                }
                Some("shutdown_response") => "Shutdown response".to_string(),
                _ => {
                    if recipient.as_deref() == Some("*") {
                        summary.map(|s| format!("Broadcast: {}", s)).unwrap_or_else(|| "Broadcast".to_string())
                    } else {
                        match (recipient, summary) {
                            (Some(r), Some(s)) => format!("To {}: {}", r, s),
                            (Some(r), None) => format!("To {}", r),
                            _ => "Send message".to_string(),
                        }
                    }
                }
            }
        }
        "TeamDelete" => "Delete team".to_string(),
        "LSP" => {
            let op = extract_field(input, "operation").unwrap_or_default();
            let file = extract_field(input, "filePath").map(|f| basename(&f));
            match (op.is_empty(), file) {
                (true, _) => "LSP".to_string(),
                (false, Some(f)) => format!("{} - {}", op, f),
                (false, None) => op,
            }
        }
        "TodoWrite" => {
            input.get("todos").and_then(|t| t.as_array()).map(|arr| {
                let count = arr.len();
                if count == 1 { "1 item".to_string() } else { format!("{} items", count) }
            }).unwrap_or_else(|| "TodoWrite".to_string())
        }
        "NotebookEdit" => {
            let file = extract_field(input, "notebook_path").map(|f| basename(&f));
            let mode = extract_field(input, "edit_mode");
            match (file, mode) {
                (Some(f), Some(m)) => format!("{} - {}", m, f),
                (Some(f), None) => f,
                (None, _) => "NotebookEdit".to_string(),
            }
        }
        // --- Default fallback ---
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

    // Team tools
    #[test]
    fn test_team_create() {
        let input = json!({"team_name": "my-team", "description": "Build feature X"});
        assert_eq!(get_tool_summary("TeamCreate", &input), "my-team - Build feature X");
    }

    #[test]
    fn test_task_create() {
        let input = json!({"subject": "Implement auth flow"});
        assert_eq!(get_tool_summary("TaskCreate", &input), "Implement auth flow");
    }

    #[test]
    fn test_task_update() {
        let input = json!({"taskId": "task-1", "status": "in_progress", "owner": "researcher"});
        assert_eq!(get_tool_summary("TaskUpdate", &input), "#task-1 in_progress -> researcher");
    }

    #[test]
    fn test_task_list() {
        assert_eq!(get_tool_summary("TaskList", &json!({})), "List tasks");
    }

    #[test]
    fn test_task_get() {
        let input = json!({"taskId": "task-42"});
        assert_eq!(get_tool_summary("TaskGet", &input), "Get task #task-42");
    }

    #[test]
    fn test_send_message_dm() {
        let input = json!({"recipient": "researcher", "summary": "Start working on X"});
        assert_eq!(get_tool_summary("SendMessage", &input), "To researcher: Start working on X");
    }

    #[test]
    fn test_send_message_shutdown() {
        let input = json!({"type": "shutdown_request", "recipient": "researcher"});
        assert_eq!(get_tool_summary("SendMessage", &input), "Shutdown researcher");
    }

    #[test]
    fn test_send_message_broadcast() {
        let input = json!({"recipient": "*", "summary": "Team update"});
        assert_eq!(get_tool_summary("SendMessage", &input), "Broadcast: Team update");
    }

    #[test]
    fn test_team_delete() {
        assert_eq!(get_tool_summary("TeamDelete", &json!({})), "Delete team");
    }

    #[test]
    fn test_lsp() {
        let input = json!({"operation": "goToDefinition", "filePath": "/src/main.rs"});
        assert_eq!(get_tool_summary("LSP", &input), "goToDefinition - main.rs");
    }

    #[test]
    fn test_todo_write() {
        let input = json!({"todos": [{"a": 1}, {"b": 2}, {"c": 3}]});
        assert_eq!(get_tool_summary("TodoWrite", &input), "3 items");
    }

    #[test]
    fn test_notebook_edit() {
        let input = json!({"notebook_path": "/work/analysis.ipynb", "edit_mode": "insert"});
        assert_eq!(get_tool_summary("NotebookEdit", &input), "insert - analysis.ipynb");
    }

    // Enhanced existing tools
    #[test]
    fn test_edit_with_line_counts() {
        let input = json!({
            "file_path": "/src/main.rs",
            "old_string": "line1\nline2\nline3",
            "new_string": "line1\nline2\nline3\nline4\nline5"
        });
        assert_eq!(get_tool_summary("Edit", &input), "main.rs - 3 -> 5 lines");
    }

    #[test]
    fn test_edit_same_line_count() {
        let input = json!({
            "file_path": "/src/main.rs",
            "old_string": "a\nb\nc",
            "new_string": "x\ny\nz"
        });
        assert_eq!(get_tool_summary("Edit", &input), "main.rs - 3 lines");
    }

    #[test]
    fn test_read_with_limit() {
        let input = json!({"file_path": "/src/main.rs", "limit": 100, "offset": 50});
        assert_eq!(get_tool_summary("Read", &input), "main.rs - lines 51-150");
    }

    #[test]
    fn test_write_with_content() {
        let input = json!({"file_path": "/src/main.rs", "content": "line1\nline2\nline3"});
        assert_eq!(get_tool_summary("Write", &input), "main.rs - 3 lines");
    }

    #[test]
    fn test_grep_with_glob() {
        let input = json!({"pattern": "TODO", "glob": "**/*.rs"});
        assert_eq!(get_tool_summary("Grep", &input), r#""TODO" in **/*.rs"#);
    }

    #[test]
    fn test_glob_with_path() {
        let input = json!({"pattern": "*.test.ts", "path": "/src/components"});
        assert_eq!(get_tool_summary("Glob", &input), r#""*.test.ts" in components"#);
    }

    #[test]
    fn test_web_fetch_url_parsing() {
        let input = json!({"url": "https://example.com/api/v1/data?key=value"});
        let result = get_tool_summary("WebFetch", &input);
        assert!(result.contains("example.com"), "Expected hostname in: {}", result);
        assert!(result.contains("/api/v1/data"), "Expected pathname in: {}", result);
    }

    #[test]
    fn test_web_search_with_quotes() {
        let input = json!({"query": "rust async patterns"});
        assert_eq!(get_tool_summary("WebSearch", &input), r#""rust async patterns""#);
    }
}
