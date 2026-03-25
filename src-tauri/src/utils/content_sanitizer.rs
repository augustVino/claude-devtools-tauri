//! Content sanitization utilities aligned with Electron's contentSanitizer.ts.
//!
//! Used for title extraction and display formatting of raw JSONL content
//! that may contain XML tags injected by Claude Code.

/// Tags that represent noise and should be completely removed.
const NOISE_TAGS: &[&str] = &[
    "local-command-caveat",
    "system-reminder",
    "task-notification",
];

/// Command-related tags to strip from mixed content.
const COMMAND_TAGS: &[&str] = &["command-name", "command-message", "command-args"];

/// Remove all occurrences of a specific XML tag (including content between open/close).
fn remove_tag(text: &str, tag: &str) -> String {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let mut result = text.to_string();

    while let Some(start) = result.find(&open) {
        if let Some(end_offset) = result[start..].find(&close) {
            let end = start + end_offset + close.len();
            result.replace_range(start..end, "");
        } else {
            // Unclosed tag — remove from open tag to end
            result.truncate(start);
            break;
        }
    }

    result
}

/// Remove noise tags from content: `<local-command-caveat>`, `<system-reminder>`, `<task-notification>`.
pub fn remove_noise_tags(text: &str) -> String {
    let mut result = text.to_string();
    for tag in NOISE_TAGS {
        result = remove_tag(&result, tag);
    }
    result
}

/// Extract command display from `<command-name>` tags.
///
/// The tag content already includes the `/` prefix (e.g., `<command-name>/compact</command-name>`).
/// Returns e.g. `/model sonnet` or `/compact`.
pub fn extract_command_display(text: &str) -> Option<String> {
    let name_start = "<command-name>";
    let name_end = "</command-name>";

    let name = text.find(name_start).and_then(|s| {
        let rest = &text[s + name_start.len()..];
        rest.find(name_end).map(|e| rest[..e].trim().to_string())
    })?;

    let args_start = "<command-args>";
    let args_end = "</command-args>";
    let args = text.find(args_start).and_then(|s| {
        let rest = &text[s + args_start.len()..];
        rest.find(args_end).map(|e| rest[..e].trim().to_string())
    });

    // name already includes the "/" prefix (e.g., "/model")
    let display = match args {
        Some(ref a) if !a.is_empty() => format!("{} {}", name, a),
        _ => name,
    };

    Some(display)
}

/// Sanitize content for display (title extraction).
///
/// Matches Electron's `sanitizeDisplayContent()` behavior:
/// 1. Removes noise tags
/// 2. Removes remaining command tags
/// 3. Removes trailing "Read the output file..." instructions
/// 4. Trims whitespace
pub fn sanitize_display_content(text: &str) -> String {
    // Remove noise tags
    let mut sanitized = remove_noise_tags(text);

    // Remove remaining command tags (in case of mixed content)
    for tag in COMMAND_TAGS {
        sanitized = remove_tag(&sanitized, tag);
    }

    // Remove trailing "Read the output file to retrieve the result: /path" instructions
    if let Some(pos) = sanitized.find("Read the output file to retrieve the result:") {
        sanitized.truncate(pos);
    }

    sanitized.trim().to_string()
}

/// Check if content starts with a command output tag.
pub fn is_command_output_content(text: &str) -> bool {
    text.trim_start()
        .starts_with("<local-command-stdout>")
        || text
            .trim_start()
            .starts_with("<local-command-stderr>")
}

/// Check if content starts with a command message tag.
pub fn is_command_content(text: &str) -> bool {
    text.trim_start().starts_with("<command-name>")
        || text
            .trim_start()
            .starts_with("<command-message>")
}

/// Extract the first user message suitable for use as a session title.
///
/// Iterates through messages to find the first suitable non-meta user text,
/// with a fallback to command display text. This mirrors Electron's
/// `analyzeSessionFileMetadata` title extraction logic.
///
/// Returns `None` if no suitable message is found.
pub fn extract_session_title<'a, I>(messages: I) -> Option<String>
where
    I: Iterator<Item = &'a serde_json::Value>,
{
    let mut first_command_text: Option<String> = None;

    for msg in messages {
        // Must be a user message
        let msg_type = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if msg_type != "user" {
            continue;
        }

        // Skip meta messages
        let is_meta = msg.get("isMeta").and_then(|v| v.as_bool()).unwrap_or(false);
        if is_meta {
            continue;
        }

        // Extract text from content (string or array of text blocks)
        let text = match msg.get("message").and_then(|m| m.get("content")) {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Array(arr)) => arr
                .iter()
                .filter_map(|block| {
                    if block.get("type")?.as_str()? == "text" {
                        block.get("text")?.as_str().map(|s| s.trim().to_string())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
            _ => continue,
        };

        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Skip command output and interruptions
        if is_command_output_content(trimmed)
            || trimmed.starts_with("[Request interrupted by user")
        {
            continue;
        }

        // Store command-name as fallback, keep looking for real text
        if is_command_content(trimmed) {
            if first_command_text.is_none() {
                first_command_text = extract_command_display(trimmed);
            }
            continue;
        }

        // Real user text found — sanitize and truncate to 500 chars
        let sanitized = sanitize_display_content(trimmed);
        if sanitized.is_empty() {
            continue;
        }
        let truncated: String = sanitized.chars().take(500).collect();
        return Some(truncated);
    }

    // Fall back to stored command text
    first_command_text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remove_noise_tags_removes_caveat() {
        let input = "hello <local-command-caveat>some warning</local-command-caveat> world";
        let result = remove_noise_tags(input);
        assert_eq!(result, "hello  world");
    }

    #[test]
    fn test_remove_noise_tags_removes_system_reminder() {
        let input = "<system-reminder>instructions</system-reminder>do this";
        let result = remove_noise_tags(input);
        assert_eq!(result, "do this");
    }

    #[test]
    fn test_remove_noise_tags_removes_task_notification() {
        let input = "<task-notification>info</task-notification>real text";
        let result = remove_noise_tags(input);
        assert_eq!(result, "real text");
    }

    #[test]
    fn test_remove_noise_tags_multiple() {
        let input = "<system-reminder>sys</system-reminder>text<local-command-caveat>caveat</local-command-caveat>";
        let result = remove_noise_tags(input);
        assert_eq!(result, "text");
    }

    #[test]
    fn test_remove_noise_tags_no_tags() {
        let input = "plain text";
        assert_eq!(remove_noise_tags(input), "plain text");
    }

    #[test]
    fn test_remove_noise_tags_unclosed_tag() {
        let input = "text<local-command-caveat>unclosed";
        let result = remove_noise_tags(input);
        assert_eq!(result, "text");
    }

    #[test]
    fn test_extract_command_display_name_only() {
        let input = "<command-name>/compact</command-name><command-message>Compact context</command-message>";
        assert_eq!(extract_command_display(input), Some("/compact".to_string()));
    }

    #[test]
    fn test_extract_command_display_with_args() {
        let input = "<command-name>/model</command-name><command-args>sonnet</command-args>";
        assert_eq!(
            extract_command_display(input),
            Some("/model sonnet".to_string())
        );
    }

    #[test]
    fn test_extract_command_display_message_first() {
        let input = "<command-message>Switch model</command-message><command-name>/model</command-name><command-args>opus</command-args>";
        assert_eq!(
            extract_command_display(input),
            Some("/model opus".to_string())
        );
    }

    #[test]
    fn test_extract_command_display_no_command() {
        let input = "just regular text";
        assert_eq!(extract_command_display(input), None);
    }

    #[test]
    fn test_sanitize_display_content_noise_tags() {
        let input = "<system-reminder>sys</system-reminder>real content";
        assert_eq!(sanitize_display_content(input), "real content");
    }

    #[test]
    fn test_sanitize_display_content_command_tags() {
        let input = "before <command-name>/model</command-name> after";
        assert_eq!(sanitize_display_content(input), "before  after");
    }

    #[test]
    fn test_sanitize_display_content_trailing_instruction() {
        let input = "Some text. Read the output file to retrieve the result: /tmp/out.txt";
        assert_eq!(sanitize_display_content(input), "Some text.");
    }

    #[test]
    fn test_sanitize_display_content_trim() {
        let input = "  hello world  ";
        assert_eq!(sanitize_display_content(input), "hello world");
    }

    #[test]
    fn test_is_command_output_content_stdout() {
        assert!(is_command_output_content("<local-command-stdout>output</local-command-stdout>"));
    }

    #[test]
    fn test_is_command_output_content_stderr() {
        assert!(is_command_output_content("<local-command-stderr>error</local-command-stderr>"));
    }

    #[test]
    fn test_is_command_output_content_regular() {
        assert!(!is_command_output_content("regular text"));
    }

    #[test]
    fn test_is_command_content() {
        assert!(is_command_content("<command-name>/model</command-name>"));
        assert!(is_command_content("<command-message>msg</command-message>"));
        assert!(!is_command_content("regular text"));
    }

    #[test]
    fn test_extract_session_title_simple_text() {
        let messages = vec![
            serde_json::json!({
                "type": "user",
                "isMeta": false,
                "message": { "content": "Hello, please help me with my project" }
            }),
        ];
        assert_eq!(
            extract_session_title(messages.iter()),
            Some("Hello, please help me with my project".to_string())
        );
    }

    #[test]
    fn test_extract_session_title_skips_meta() {
        let messages = vec![
            serde_json::json!({
                "type": "user",
                "isMeta": true,
                "message": { "content": "internal meta" }
            }),
            serde_json::json!({
                "type": "user",
                "isMeta": false,
                "message": { "content": "real user message" }
            }),
        ];
        assert_eq!(
            extract_session_title(messages.iter()),
            Some("real user message".to_string())
        );
    }

    #[test]
    fn test_extract_session_title_skips_command_output() {
        let messages = vec![
            serde_json::json!({
                "type": "user",
                "isMeta": false,
                "message": { "content": "<local-command-stdout>output</local-command-stdout>" }
            }),
            serde_json::json!({
                "type": "user",
                "isMeta": false,
                "message": { "content": "real message" }
            }),
        ];
        assert_eq!(
            extract_session_title(messages.iter()),
            Some("real message".to_string())
        );
    }

    #[test]
    fn test_extract_session_title_command_fallback() {
        let messages = vec![
            serde_json::json!({
                "type": "user",
                "isMeta": false,
                "message": { "content": "<command-name>/model</command-name><command-args>sonnet</command-args>" }
            }),
        ];
        assert_eq!(
            extract_session_title(messages.iter()),
            Some("/model sonnet".to_string())
        );
    }

    #[test]
    fn test_extract_session_title_command_keeps_looking() {
        let messages = vec![
            serde_json::json!({
                "type": "user",
                "isMeta": false,
                "message": { "content": "<command-name>/compact</command-name>" }
            }),
            serde_json::json!({
                "type": "user",
                "isMeta": false,
                "message": { "content": "actual user request" }
            }),
        ];
        // Should prefer real text over command
        assert_eq!(
            extract_session_title(messages.iter()),
            Some("actual user request".to_string())
        );
    }

    #[test]
    fn test_extract_session_title_array_content() {
        let messages = vec![
            serde_json::json!({
                "type": "user",
                "isMeta": false,
                "message": {
                    "content": [
                        { "type": "text", "text": "Hello " },
                        { "type": "text", "text": "world" }
                    ]
                }
            }),
        ];
        assert_eq!(
            extract_session_title(messages.iter()),
            Some("Hello world".to_string())
        );
    }

    #[test]
    fn test_extract_session_title_truncates_to_500() {
        let long_text = "a".repeat(600);
        let messages = vec![serde_json::json!({
            "type": "user",
            "isMeta": false,
            "message": { "content": long_text }
        })];
        let result = extract_session_title(messages.iter()).unwrap();
        assert_eq!(result.chars().count(), 500);
    }

    #[test]
    fn test_extract_session_title_sanitizes() {
        let messages = vec![serde_json::json!({
            "type": "user",
            "isMeta": false,
            "message": {
                "content": "<system-reminder>rules</system-reminder>Please fix the bug"
            }
        })];
        assert_eq!(
            extract_session_title(messages.iter()),
            Some("Please fix the bug".to_string())
        );
    }

    #[test]
    fn test_extract_session_title_skips_interruption() {
        let messages = vec![
            serde_json::json!({
                "type": "user",
                "isMeta": false,
                "message": { "content": "[Request interrupted by user]" }
            }),
            serde_json::json!({
                "type": "user",
                "isMeta": false,
                "message": { "content": "after interruption" }
            }),
        ];
        assert_eq!(
            extract_session_title(messages.iter()),
            Some("after interruption".to_string())
        );
    }
}
