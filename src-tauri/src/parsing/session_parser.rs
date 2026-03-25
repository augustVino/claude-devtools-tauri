use crate::parsing::jsonl_parser::{self, calculate_metrics, get_task_calls, parse_jsonl_file};
use crate::parsing::message_classifier;
use crate::types::domain::{MessageCategory, MessageType, SessionMetrics};
use crate::types::messages::{ParsedMessage, ToolCall};

/// Result of parsing a session file.
pub struct ParsedSession {
    pub messages: Vec<ParsedMessage>,
    pub metrics: SessionMetrics,
    pub task_calls: Vec<ToolCall>,
    pub by_type: MessagesByType,
    pub sidechain_messages: Vec<ParsedMessage>,
    pub main_messages: Vec<ParsedMessage>,
    pub is_ongoing: bool,
}

/// Messages categorized by type.
pub struct MessagesByType {
    pub user: Vec<ParsedMessage>,
    pub real_user: Vec<ParsedMessage>,
    pub internal_user: Vec<ParsedMessage>,
    pub assistant: Vec<ParsedMessage>,
    pub system: Vec<ParsedMessage>,
    pub other: Vec<ParsedMessage>,
}

impl ParsedSession {
    pub fn get_user_messages(&self) -> &[ParsedMessage] {
        &self.by_type.user
    }

    pub fn get_assistant_messages(&self) -> &[ParsedMessage] {
        &self.by_type.assistant
    }

    /// Get assistant responses between a user message and the next user message.
    pub fn get_responses(&self, user_message_uuid: &str) -> Vec<&ParsedMessage> {
        let mut in_range = false;
        let mut responses = vec![];

        for msg in &self.main_messages {
            if msg.uuid == user_message_uuid {
                in_range = true;
                continue;
            }
            if in_range {
                if message_classifier::is_real_user_message(msg) {
                    break;
                }
                if message_classifier::is_assistant_message(msg) {
                    responses.push(msg);
                }
            }
        }

        responses
    }
}

/// Parse a session file from disk.
pub async fn parse_session_file(file_path: &std::path::Path) -> ParsedSession {
    let messages = parse_jsonl_file(file_path).await;
    process_messages(&messages)
}

/// Process parsed messages into categorized groups.
pub fn process_messages(messages: &[ParsedMessage]) -> ParsedSession {
    let mut by_type = MessagesByType {
        user: vec![],
        real_user: vec![],
        internal_user: vec![],
        assistant: vec![],
        system: vec![],
        other: vec![],
    };
    let mut sidechain_messages = vec![];
    let mut main_messages = vec![];

    for msg in messages {
        match msg.message_type {
            MessageType::User => {
                by_type.user.push(msg.clone());
                if message_classifier::is_real_user_message(msg) {
                    by_type.real_user.push(msg.clone());
                } else if message_classifier::is_internal_user_message(msg) {
                    by_type.internal_user.push(msg.clone());
                }
            }
            MessageType::Assistant => {
                by_type.assistant.push(msg.clone());
            }
            MessageType::System => {
                by_type.system.push(msg.clone());
            }
            _ => {
                by_type.other.push(msg.clone());
            }
        }

        if msg.is_sidechain {
            sidechain_messages.push(msg.clone());
        } else {
            main_messages.push(msg.clone());
        }
    }

    let metrics = calculate_metrics(messages);
    let task_calls = get_task_calls(messages);
    let is_ongoing = crate::utils::session_state_detection::check_messages_ongoing(messages);

    ParsedSession {
        messages: messages.to_vec(),
        metrics,
        task_calls,
        by_type,
        sidechain_messages,
        main_messages,
        is_ongoing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::jsonl::UsageMetadata;

    fn make_user_msg(uuid: &str, is_meta: bool, is_sidechain: bool) -> ParsedMessage {
        ParsedMessage {
            uuid: uuid.to_string(),
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
            is_sidechain,
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

    fn make_assistant_msg(uuid: &str) -> ParsedMessage {
        ParsedMessage {
            uuid: uuid.to_string(),
            parent_uuid: None,
            message_type: MessageType::Assistant,
            timestamp: "2026-01-01T00:00:01Z".to_string(),
            role: Some("assistant".to_string()),
            content: serde_json::Value::String("response".to_string()),
            usage: Some(UsageMetadata {
                input_tokens: 100,
                output_tokens: 50,
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            }),
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

    #[test]
    fn test_process_messages_categorization() {
        let messages = vec![
            make_user_msg("u1", false, false),
            make_assistant_msg("a1"),
            make_user_msg("u2", true, false),
            make_assistant_msg("a2"),
        ];

        let session = process_messages(&messages);
        assert_eq!(session.by_type.user.len(), 2);
        assert_eq!(session.by_type.real_user.len(), 1);
        assert_eq!(session.by_type.internal_user.len(), 1);
        assert_eq!(session.by_type.assistant.len(), 2);
        assert_eq!(session.main_messages.len(), 4);
    }

    #[test]
    fn test_process_messages_sidechain() {
        let messages = vec![
            make_user_msg("u1", false, false),
            make_user_msg("u2", true, true), // sidechain
        ];

        let session = process_messages(&messages);
        assert_eq!(session.main_messages.len(), 1);
        assert_eq!(session.sidechain_messages.len(), 1);
    }

    #[test]
    fn test_process_messages_metrics() {
        let messages = vec![
            make_user_msg("u1", false, false),
            make_assistant_msg("a1"),
        ];

        let session = process_messages(&messages);
        assert_eq!(session.metrics.input_tokens, 100);
        assert_eq!(session.metrics.output_tokens, 50);
        assert_eq!(session.metrics.message_count, 2);
    }

    #[test]
    fn test_get_responses() {
        let messages = vec![
            make_user_msg("u1", false, false),
            make_assistant_msg("a1"),
            make_assistant_msg("a2"),
            make_user_msg("u2", false, false),
            make_assistant_msg("a3"),
        ];

        let session = process_messages(&messages);
        let responses = session.get_responses("u1");
        assert_eq!(responses.len(), 2); // a1 and a2
    }
}
