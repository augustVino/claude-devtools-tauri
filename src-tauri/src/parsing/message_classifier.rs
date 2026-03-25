use crate::types::domain::MessageCategory;
use crate::types::messages::ParsedMessage;

/// Classify a message into a category based on priority-ordered type guards.
///
/// Priority order:
/// 1. hardNoise — system metadata, caveats, reminders (filtered out)
/// 2. compact — summary messages from conversation compaction
/// 3. system — command output (<local-command-stdout>)
/// 4. user — genuine user input (creates UserChunk)
/// 5. ai — fallback: assistant messages, tool results, etc.
pub fn classify_message(msg: &ParsedMessage) -> MessageCategory {
    if is_hard_noise(msg) {
        return MessageCategory::HardNoise;
    }
    if is_compact(msg) {
        return MessageCategory::Compact;
    }
    if is_system_chunk(msg) {
        return MessageCategory::System;
    }
    if is_user_chunk(msg) {
        return MessageCategory::User;
    }
    MessageCategory::Ai
}

/// Classify all messages into categories.
pub fn classify_messages(messages: &[ParsedMessage]) -> Vec<(MessageCategory, &ParsedMessage)> {
    messages.iter().map(|m| (classify_message(m), m)).collect()
}

// =============================================================================
// Type Guard Functions
// =============================================================================

/// Real user message: type=user, isMeta=false, string or array content with text.
pub fn is_real_user_message(msg: &ParsedMessage) -> bool {
    if msg.message_type != crate::types::domain::MessageType::User {
        return false;
    }
    if msg.is_meta {
        return false;
    }
    true
}

/// Internal user message: type=user, isMeta=true.
pub fn is_internal_user_message(msg: &ParsedMessage) -> bool {
    msg.message_type == crate::types::domain::MessageType::User && msg.is_meta
}

/// Assistant message check.
pub fn is_assistant_message(msg: &ParsedMessage) -> bool {
    msg.message_type == crate::types::domain::MessageType::Assistant
}

/// Hard noise: system messages that are metadata (caveats, reminders, etc.)
fn is_hard_noise(msg: &ParsedMessage) -> bool {
    if msg.message_type != crate::types::domain::MessageType::System {
        return false;
    }
    if !msg.is_meta {
        return false;
    }
    // Check content for known noise patterns
    let text = extract_content_text(&msg.content);
    let noise_keywords = [
        "<system-reminder>",
        "CLAUDE.md",
        "IMPORTANT:",
        "thinking",
        "CRITICAL:",
    ];
    noise_keywords.iter().any(|kw| text.contains(kw))
}

/// Compact message: summary entries or user entries with isCompactSummary flag.
fn is_compact(msg: &ParsedMessage) -> bool {
    if msg.message_type == crate::types::domain::MessageType::Summary {
        return true;
    }
    if msg.is_compact_summary.unwrap_or(false) {
        return true;
    }
    false
}

/// System chunk: system messages with command output content.
fn is_system_chunk(msg: &ParsedMessage) -> bool {
    if msg.message_type != crate::types::domain::MessageType::System {
        return false;
    }
    // Command output is indicated by <local-command-stdout> or similar tags
    let text = extract_content_text(&msg.content);
    text.contains("<local-command-stdout>") || text.contains("command-output")
}

/// User chunk: real user messages that create chunks.
fn is_user_chunk(msg: &ParsedMessage) -> bool {
    is_real_user_message(msg)
}

/// Extract text from content value.
fn extract_content_text(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|block| {
                if block.get("type")?.as_str()? == "text" {
                    block.get("text")?.as_str().map(String::from)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::domain::MessageType;

    fn make_user_msg(is_meta: bool) -> ParsedMessage {
        ParsedMessage {
            uuid: "u1".to_string(),
            parent_uuid: None,
            message_type: MessageType::User,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            role: Some("user".to_string()),
            content: serde_json::Value::String("hello".to_string()),
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

    fn make_assistant_msg() -> ParsedMessage {
        ParsedMessage {
            uuid: "a1".to_string(),
            parent_uuid: None,
            message_type: MessageType::Assistant,
            timestamp: "2026-01-01T00:00:01Z".to_string(),
            role: Some("assistant".to_string()),
            content: serde_json::Value::String("response".to_string()),
            usage: None,
            model: Some("claude-3-opus".to_string()),
            cwd: None,
            git_branch: None,
            agent_id: None,
            is_sidechain: false,
            is_meta: false,
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

    fn make_system_msg(is_meta: bool, content: &str) -> ParsedMessage {
        ParsedMessage {
            uuid: "s1".to_string(),
            parent_uuid: None,
            message_type: MessageType::System,
            timestamp: "2026-01-01T00:00:02Z".to_string(),
            role: None,
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
    fn test_classify_real_user() {
        let msg = make_user_msg(false);
        assert_eq!(classify_message(&msg), MessageCategory::User);
        assert!(is_real_user_message(&msg));
    }

    #[test]
    fn test_classify_internal_user() {
        let msg = make_user_msg(true);
        assert_eq!(classify_message(&msg), MessageCategory::Ai);
    }

    #[test]
    fn test_classify_assistant() {
        let msg = make_assistant_msg();
        assert_eq!(classify_message(&msg), MessageCategory::Ai);
        assert!(is_assistant_message(&msg));
    }

    #[test]
    fn test_classify_hard_noise() {
        let msg = make_system_msg(true, "<system-reminder>Some reminder</system-reminder>");
        assert_eq!(classify_message(&msg), MessageCategory::HardNoise);
    }

    #[test]
    fn test_classify_system_chunk() {
        let msg = make_system_msg(false, "<local-command-stdout>ls output</local-command-stdout>");
        assert_eq!(classify_message(&msg), MessageCategory::System);
    }
}
