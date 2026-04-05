//! 内容清洗工具模块，与 Electron 端 contentSanitizer.ts 保持一致。
//!
//! 用于从原始 JSONL 内容中提取标题和格式化显示文本，
//! 处理 Claude Code 注入的 XML 标签。

use crate::types::messages::ParsedMessage;

/// 需要完全移除的噪声标签列表。
const NOISE_TAGS: &[&str] = &[
    "local-command-caveat",
    "system-reminder",
    "task-notification",
];

/// 混合内容中需要剥离的命令相关标签列表。
const COMMAND_TAGS: &[&str] = &["command-name", "command-message", "command-args"];

/// 移除指定 XML 标签及其内容（包括开闭标签之间的所有文本）。
fn remove_tag(text: &str, tag: &str) -> String {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let mut result = text.to_string();

    while let Some(start) = result.find(&open) {
        if let Some(end_offset) = result[start..].find(&close) {
            let end = start + end_offset + close.len();
            result.replace_range(start..end, "");
        } else {
            // 未闭合标签 — 从开标签处截断到末尾
            result.truncate(start);
            break;
        }
    }

    result
}

/// 从内容中移除噪声标签：`<local-command-caveat>`、`<system-reminder>`、`<task-notification>`。
pub fn remove_noise_tags(text: &str) -> String {
    let mut result = text.to_string();
    for tag in NOISE_TAGS {
        result = remove_tag(&result, tag);
    }
    result
}

/// 从 `<command-name>` 标签中提取命令显示文本。
///
/// 标签内容已包含 `/` 前缀（如 `<command-name>/compact</command-name>`）。
/// 返回值示例：`/model sonnet` 或 `/compact`。
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

    // name 已包含 "/" 前缀（如 "/model"）
    let display = match args {
        Some(ref a) if !a.is_empty() => format!("{} {}", name, a),
        _ => name,
    };

    Some(display)
}

/// 清洗内容用于显示（标题提取）。
///
/// 与 Electron 端 `sanitizeDisplayContent()` 行为一致：
/// 1. 移除噪声标签
/// 2. 移除剩余的命令标签
/// 3. 移除末尾的 "Read the output file..." 指令
/// 4. 去除首尾空白
pub fn sanitize_display_content(text: &str) -> String {
    // 如果内容是命令消息，提取命令显示文本后直接返回（与 Electron 对齐）。
    // Electron 的 sanitizeDisplayContent() 会在移除噪声标签之前检查 isCommandContent()
    // 并返回 extractCommandDisplay() 的结果，避免将命令标签全部剥离后返回空字符串。
    if is_command_content(text) {
        if let Some(display) = extract_command_display(text) {
            return display;
        }
    }

    // 第一步：移除噪声标签
    let mut sanitized = remove_noise_tags(text);

    // 第二步：移除剩余的命令标签（处理混合内容场景）
    for tag in COMMAND_TAGS {
        sanitized = remove_tag(&sanitized, tag);
    }

    // 第三步：移除末尾的 "Read the output file to retrieve the result: /path" 指令
    if let Some(pos) = sanitized.find("Read the output file to retrieve the result:") {
        sanitized.truncate(pos);
    }

    sanitized.trim().to_string()
}

/// 检查内容是否以命令输出标签开头。
pub fn is_command_output_content(text: &str) -> bool {
    text.trim_start()
        .starts_with("<local-command-stdout>")
        || text
            .trim_start()
            .starts_with("<local-command-stderr>")
}

/// 检查内容是否以命令消息标签开头。
pub fn is_command_content(text: &str) -> bool {
    text.trim_start().starts_with("<command-name>")
        || text
            .trim_start()
            .starts_with("<command-message>")
}

/// 从消息序列中提取第一条适合作为会话标题的用户消息。
///
/// 遍历消息列表，查找第一条合适的非元用户文本，
/// 回退到命令显示文本。此逻辑镜像 Electron 端
/// `analyzeSessionFileMetadata` 的标题提取行为。
///
/// 若未找到合适的消息则返回 `None`。
#[allow(dead_code)]
pub fn extract_session_title<'a, I>(messages: I) -> Option<String>
where
    I: Iterator<Item = &'a serde_json::Value>,
{
    let mut first_command_text: Option<String> = None;

    for msg in messages {
        // 仅处理用户消息
        let msg_type = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if msg_type != "user" {
            continue;
        }

        // 跳过元消息
        let is_meta = msg.get("isMeta").and_then(|v| v.as_bool()).unwrap_or(false);
        if is_meta {
            continue;
        }

        // 从 content 中提取文本（支持字符串和文本块数组两种格式）
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

        // 跳过命令输出和用户中断消息
        if is_command_output_content(trimmed)
            || trimmed.starts_with("[Request interrupted by user")
        {
            continue;
        }

        // 存储命令文本作为回退选项，继续查找真实用户文本
        if is_command_content(trimmed) {
            if first_command_text.is_none() {
                first_command_text = extract_command_display(trimmed);
            }
            continue;
        }

        // 找到真实用户文本 — 清洗并截断至 500 字符
        let sanitized = sanitize_display_content(trimmed);
        if sanitized.is_empty() {
            continue;
        }
        let truncated: String = sanitized.chars().take(500).collect();
        return Some(truncated);
    }

    // 回退到已存储的命令文本
    first_command_text
}

/// 从已解析的 `ParsedMessage` 列表中提取会话标题。
///
/// 查找第一条真实的用户消息文本（type=user, is_meta=false），
/// 截取前 500 字符作为标题。
pub fn extract_session_title_from_parsed(messages: &[ParsedMessage]) -> Option<String> {
    use crate::parsing::jsonl_parser::extract_text_content;
    use crate::parsing::message_classifier::is_real_user_message;

    for msg in messages {
        if is_real_user_message(msg) {
            let text = extract_text_content(msg);
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                let title = if trimmed.len() > 500 {
                    trimmed[..500].to_string()
                } else {
                    trimmed.to_string()
                };
                return Some(title);
            }
        }
    }
    None
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
    fn test_sanitize_display_content_command_tags_starting() {
        // Command content starting with <command-name> returns extracted display (Electron behavior)
        let input = "<command-name>/model</command-name><command-args>sonnet</command-args>";
        assert_eq!(sanitize_display_content(input), "/model sonnet");
    }

    #[test]
    fn test_sanitize_display_content_command_message_starting() {
        // Command content starting with <command-message> also returns extracted display
        let input = "<command-message>Switch model</command-message><command-name>/model</command-name><command-args>opus</command-args>";
        assert_eq!(sanitize_display_content(input), "/model opus");
    }

    #[test]
    fn test_sanitize_display_content_command_tags_mid_content() {
        // Command tags in the middle of other content (not starting) get stripped
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
        // 优先选择真实文本而非命令
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

    // =========================================================================
    // extract_session_title_from_parsed tests
    // =========================================================================

    fn make_parsed_user_msg(is_meta: bool, content: &str) -> ParsedMessage {
        ParsedMessage {
            uuid: "u1".to_string(),
            parent_uuid: None,
            message_type: crate::types::domain::MessageType::User,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            role: Some("user".to_string()),
            content: serde_json::Value::String(content.to_string()),
            usage: None,
            model: None,
            cwd: None,
            git_branch: None,
            agent_id: None,
            is_sidechain: false,
            is_meta,
            user_type: None,
            tool_calls: vec![],
            tool_results: vec![],
            source_tool_use_id: None,
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            is_compact_summary: None,
            request_id: None,
        }
    }

    #[test]
    fn test_extract_session_title_from_parsed_basic() {
        let messages = vec![
            make_parsed_user_msg(false, "Hello, please help me with my project"),
        ];
        assert_eq!(
            extract_session_title_from_parsed(&messages),
            Some("Hello, please help me with my project".to_string())
        );
    }

    #[test]
    fn test_extract_session_title_from_parsed_skips_meta() {
        let messages = vec![
            make_parsed_user_msg(true, "internal meta message"),
            make_parsed_user_msg(false, "real user message"),
        ];
        assert_eq!(
            extract_session_title_from_parsed(&messages),
            Some("real user message".to_string())
        );
    }

    #[test]
    fn test_extract_session_title_from_parsed_truncates() {
        let long_text = "a".repeat(600);
        let messages = vec![
            make_parsed_user_msg(false, &long_text),
        ];
        let result = extract_session_title_from_parsed(&messages).unwrap();
        assert_eq!(result.len(), 500);
        assert_eq!(result, "a".repeat(500));
    }
}
