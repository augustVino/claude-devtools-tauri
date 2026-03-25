use crate::types::domain::{MessageCategory, MessageType};
use crate::types::messages::ParsedMessage;

/// Classify a message into a category based on priority-ordered type guards.
///
/// Priority order (aligned with Electron's MessageClassifier):
/// 1. hardNoise — ALL system/summary messages, synthetic assistant, user noise tags, interruption
/// 2. compact — user messages with isCompactSummary flag
/// 3. system — USER messages with command output tags (<local-command-stdout>)
/// 4. user — genuine user input (type=user, !meta, !teammate, valid content)
/// 5. ai — fallback: assistant messages, meta user messages, etc.
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

/// Real user message: type=user, isMeta=false.
pub fn is_real_user_message(msg: &ParsedMessage) -> bool {
    msg.message_type == MessageType::User && !msg.is_meta
}

/// Internal user message: type=user, isMeta=true.
pub fn is_internal_user_message(msg: &ParsedMessage) -> bool {
    msg.message_type == MessageType::User && msg.is_meta
}

/// Assistant message check.
pub fn is_assistant_message(msg: &ParsedMessage) -> bool {
    msg.message_type == MessageType::Assistant
}

// =============================================================================
// Hard Noise (aligned with Electron's isParsedHardNoiseMessage)
// =============================================================================

/// Hard noise: messages that should NEVER be rendered or counted.
/// Matches Electron: all system/summary types, synthetic assistant,
/// user messages with only noise tags, and interruptions.
fn is_hard_noise(msg: &ParsedMessage) -> bool {
    // 1. All system messages are hard noise (Electron: type === 'system' → true)
    if msg.message_type == MessageType::System {
        return true;
    }

    // 2. All summary messages are hard noise (Electron: type === 'summary' → true)
    if msg.message_type == MessageType::Summary {
        return true;
    }

    // 3. Synthetic assistant messages (Electron: model === '<synthetic>')
    if msg.message_type == MessageType::Assistant {
        if msg.model.as_deref() == Some("<synthetic>") {
            return true;
        }
    }

    // 4. User messages with only noise tags or interruptions
    if msg.message_type == MessageType::User {
        let text = extract_content_text(&msg.content);
        let trimmed = text.trim();

        // Empty command output
        if trimmed == "<local-command-stdout></local-command-stdout>"
            || trimmed == "<local-command-stderr></local-command-stderr>"
        {
            return true;
        }

        // Content wrapped entirely in noise tags
        for tag in ["<local-command-caveat>", "<system-reminder>"] {
            let close_tag = tag.replace("<", "</");
            if trimmed.starts_with(tag) && trimmed.ends_with(&close_tag) {
                return true;
            }
        }

        // Interruption messages
        if trimmed.starts_with("[Request interrupted by user") {
            return true;
        }
    }

    false
}

// =============================================================================
// Compact (aligned with Electron's isParsedCompactMessage)
// =============================================================================

/// Compact message: user messages with isCompactSummary flag.
/// NOTE: In Electron, summary-type messages are caught by hardNoise first,
/// so Compact only comes from user messages with isCompactSummary === true.
fn is_compact(msg: &ParsedMessage) -> bool {
    msg.is_compact_summary.unwrap_or(false)
}

// =============================================================================
// System Chunk (aligned with Electron's isParsedSystemChunkMessage)
// =============================================================================

/// System chunk: USER messages with command output tags.
/// In the JSONL format, command output arrives as user entries
/// (tool results wrapped in <local-command-stdout> tags), NOT as system entries.
fn is_system_chunk(msg: &ParsedMessage) -> bool {
    if msg.message_type != MessageType::User {
        return false;
    }

    let text = extract_content_text(&msg.content);

    // String content check
    if text.starts_with("<local-command-stdout>") || text.starts_with("<local-command-stderr>") {
        return true;
    }

    // Array content check — any text block starting with the tag
    if let serde_json::Value::Array(arr) = &msg.content {
        return arr.iter().any(|block| {
            let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if block_type != "text" {
                return false;
            }
            let block_text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
            block_text.starts_with("<local-command-stdout>")
        });
    }

    false
}

// =============================================================================
// User Chunk (aligned with Electron's isParsedUserChunkMessage)
// =============================================================================

/// User chunk: real user messages that create visible chunks.
/// Excludes meta messages, teammate messages, system output tags, and empty content.
fn is_user_chunk(msg: &ParsedMessage) -> bool {
    if msg.message_type != MessageType::User {
        return false;
    }
    if msg.is_meta {
        return false;
    }

    // Exclude teammate messages
    if is_teammate_message(msg) {
        return false;
    }

    let content = &msg.content;

    // String content validation
    if let serde_json::Value::String(s) = content {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return false;
        }
        // Exclude system output tags
        for tag in [
            "<local-command-stderr>",
            "<local-command-stdout>",
            "<local-command-caveat>",
            "<system-reminder>",
        ] {
            if trimmed.starts_with(tag) {
                return false;
            }
        }
        return true;
    }

    // Array content — must contain text or image blocks
    if let serde_json::Value::Array(arr) = content {
        let has_user_content = arr.iter().any(|block| {
            let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
            block_type == "text" || block_type == "image"
        });
        return has_user_content;
    }

    false
}

// =============================================================================
// Teammate Message Detection
// =============================================================================

/// Detect teammate messages: <teammate-message teammate_id="name" ...>content</teammate-message>
fn is_teammate_message(msg: &ParsedMessage) -> bool {
    if msg.message_type != MessageType::User || msg.is_meta {
        return false;
    }

    let text = extract_content_text(&msg.content);
    text.trim().starts_with("<teammate-message ")
}

// =============================================================================
// Content Helpers
// =============================================================================

/// Extract text from content value (string or array of text blocks).
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

    fn make_user_msg(is_meta: bool, content: &str) -> ParsedMessage {
        ParsedMessage {
            uuid: "u1".to_string(),
            parent_uuid: None,
            message_type: MessageType::User,
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

    fn make_assistant_msg(model: &str) -> ParsedMessage {
        ParsedMessage {
            uuid: "a1".to_string(),
            parent_uuid: None,
            message_type: MessageType::Assistant,
            timestamp: "2026-01-01T00:00:01Z".to_string(),
            role: Some("assistant".to_string()),
            content: serde_json::Value::String("response".to_string()),
            usage: None,
            model: Some(model.to_string()),
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

    // --- User classification ---

    #[test]
    fn test_classify_real_user() {
        let msg = make_user_msg(false, "hello world");
        assert_eq!(classify_message(&msg), MessageCategory::User);
    }

    #[test]
    fn test_classify_internal_user_as_ai() {
        let msg = make_user_msg(true, "tool result");
        assert_eq!(classify_message(&msg), MessageCategory::Ai);
    }

    #[test]
    fn test_classify_empty_user_as_noise() {
        let msg = make_user_msg(false, "");
        // Empty string doesn't pass user chunk validation, falls to AI
        // This matches Electron behavior where empty content returns false from isParsedUserChunkMessage
        assert_ne!(classify_message(&msg), MessageCategory::User);
    }

    // --- Assistant classification ---

    #[test]
    fn test_classify_assistant_as_ai() {
        let msg = make_assistant_msg("claude-3-opus");
        assert_eq!(classify_message(&msg), MessageCategory::Ai);
    }

    #[test]
    fn test_classify_synthetic_assistant_as_noise() {
        let msg = make_assistant_msg("<synthetic>");
        assert_eq!(classify_message(&msg), MessageCategory::HardNoise);
    }

    // --- System classification (ALL system → hardNoise) ---

    #[test]
    fn test_all_system_messages_are_hard_noise() {
        let meta_system = make_system_msg(true, "<system-reminder>reminder</system-reminder>");
        let non_meta_system = make_system_msg(false, "some system info");
        let cmd_system = make_system_msg(false, "<local-command-stdout>output</local-command-stdout>");

        assert_eq!(classify_message(&meta_system), MessageCategory::HardNoise);
        assert_eq!(classify_message(&non_meta_system), MessageCategory::HardNoise);
        assert_eq!(classify_message(&cmd_system), MessageCategory::HardNoise);
    }

    // --- System chunk: USER messages with command output tags ---

    #[test]
    fn test_user_with_stdout_is_system_chunk() {
        let msg = make_user_msg(true, "<local-command-stdout>ls output\nfile1.txt\n</local-command-stdout>");
        assert_eq!(classify_message(&msg), MessageCategory::System);
    }

    #[test]
    fn test_user_with_stderr_is_system_chunk() {
        let msg = make_user_msg(true, "<local-command-stderr>error message</local-command-stderr>");
        assert_eq!(classify_message(&msg), MessageCategory::System);
    }

    #[test]
    fn test_user_with_array_stdout_is_system_chunk() {
        let msg = ParsedMessage {
            uuid: "u1".to_string(),
            parent_uuid: None,
            message_type: MessageType::User,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            role: Some("user".to_string()),
            content: serde_json::json!([
                {"type": "text", "text": "<local-command-stdout>output</local-command-stdout>"}
            ]),
            usage: None,
            model: None,
            cwd: None,
            git_branch: None,
            agent_id: None,
            is_sidechain: false,
            is_meta: true,
            user_type: None,
            tool_calls: vec![],
            tool_results: vec![],
            source_tool_use_id: None,
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            is_compact_summary: None,
            request_id: None,
        };
        assert_eq!(classify_message(&msg), MessageCategory::System);
    }

    // --- Hard noise: user messages with noise tags ---

    #[test]
    fn test_user_with_caveat_is_noise() {
        let msg = make_user_msg(false, "<local-command-caveat>Some caveat</local-command-caveat>");
        assert_eq!(classify_message(&msg), MessageCategory::HardNoise);
    }

    #[test]
    fn test_user_with_system_reminder_is_noise() {
        let msg = make_user_msg(false, "<system-reminder>Some reminder</system-reminder>");
        assert_eq!(classify_message(&msg), MessageCategory::HardNoise);
    }

    #[test]
    fn test_user_with_empty_stdout_is_noise() {
        let msg = make_user_msg(false, "<local-command-stdout></local-command-stdout>");
        assert_eq!(classify_message(&msg), MessageCategory::HardNoise);
    }

    #[test]
    fn test_user_with_interruption_is_noise() {
        let msg = make_user_msg(false, "[Request interrupted by user");
        assert_eq!(classify_message(&msg), MessageCategory::HardNoise);
    }

    // --- User chunk excludes system output tags ---

    #[test]
    fn test_user_with_stdout_prefix_not_user() {
        let msg = make_user_msg(false, "<local-command-stdout>some output</local-command-stdout>");
        // meta=true, so this should be System chunk (checked before User)
        assert_ne!(classify_message(&msg), MessageCategory::User);
    }

    // --- Teammate messages ---

    #[test]
    fn test_teammate_message_is_not_user() {
        let msg = make_user_msg(false, "<teammate-message teammate_id=\"agent-1\" color=\"blue\" summary=\"done\">result</teammate-message>");
        assert_ne!(classify_message(&msg), MessageCategory::User);
        // Falls to AI
        assert_eq!(classify_message(&msg), MessageCategory::Ai);
    }

    // --- Compact ---

    #[test]
    fn test_compact_summary_flag() {
        let mut msg = make_user_msg(true, "compacted summary");
        msg.is_compact_summary = Some(true);
        assert_eq!(classify_message(&msg), MessageCategory::Compact);
    }
}
