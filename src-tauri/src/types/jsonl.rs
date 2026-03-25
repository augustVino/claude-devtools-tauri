use serde::{Deserialize, Serialize};

// =============================================================================
// Content Blocks
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        #[serde(default, deserialize_with = "deserialize_content")]
        content: serde_json::Value,
        #[serde(default)]
        is_error: bool,
    },
    Image {
        source: ImageSource,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub media_type: String,
    pub data: String,
}

fn deserialize_content<'de, D>(deserializer: D) -> Result<serde_json::Value, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde_json::Value::deserialize(deserializer)
}

// =============================================================================
// Usage Metadata
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct UsageMetadata {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
}

// =============================================================================
// Messages
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserMessage {
    pub role: String,
    #[serde(default, deserialize_with = "deserialize_user_content")]
    pub content: serde_json::Value,
}

fn deserialize_user_content<'de, D>(deserializer: D) -> Result<serde_json::Value, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde_json::Value::deserialize(deserializer)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AssistantMessage {
    pub role: String,
    pub id: String,
    #[serde(rename = "type")]
    pub message_type: String,
    pub model: String,
    pub content: Vec<ContentBlock>,
    #[serde(default, deserialize_with = "deserialize_stop_reason")]
    pub stop_reason: Option<String>,
    #[serde(default)]
    pub stop_sequence: Option<String>,
    pub usage: UsageMetadata,
}

fn deserialize_stop_reason<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: Option<serde_json::Value> = Option::deserialize(deserializer)?;
    match value {
        None => Ok(None),
        Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(s)),
        _ => Ok(None),
    }
}

// =============================================================================
// Entry Types (JSONL wire format)
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ChatHistoryEntry {
    #[serde(rename = "user")]
    User(UserEntry),
    #[serde(rename = "assistant")]
    Assistant(AssistantEntry),
    #[serde(rename = "system")]
    System(SystemEntry),
    #[serde(rename = "summary")]
    Summary(SummaryEntry),
    #[serde(rename = "file-history-snapshot")]
    FileHistorySnapshot(FileHistorySnapshotEntry),
    #[serde(rename = "queue-operation")]
    QueueOperation(QueueOperationEntry),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UserEntry {
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub parent_uuid: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    pub message: UserMessage,
    #[serde(default)]
    pub is_meta: Option<bool>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub tool_use_result: Option<serde_json::Value>,
    #[serde(default, rename = "isCompactSummary")]
    pub is_compact_summary: Option<bool>,
    #[serde(default)]
    pub source_tool_use_id: Option<String>,
    #[serde(default)]
    pub source_tool_assistant_uuid: Option<String>,
    // ConversationalEntry fields
    #[serde(default)]
    pub is_sidechain: bool,
    #[serde(default)]
    pub user_type: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub git_branch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AssistantEntry {
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub parent_uuid: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    pub message: AssistantMessage,
    #[serde(default)]
    pub request_id: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    // ConversationalEntry fields
    #[serde(default)]
    pub is_sidechain: bool,
    #[serde(default)]
    pub user_type: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub git_branch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SystemEntry {
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub parent_uuid: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    pub subtype: String,
    pub duration_ms: u64,
    pub is_meta: bool,
    // ConversationalEntry fields
    #[serde(default)]
    pub is_sidechain: bool,
    #[serde(default)]
    pub user_type: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub git_branch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SummaryEntry {
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub timestamp: Option<String>,
    pub summary: String,
    pub leaf_uuid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileHistorySnapshotEntry {
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub timestamp: Option<String>,
    pub message_id: String,
    pub snapshot: serde_json::Value,
    #[serde(default)]
    pub is_snapshot_update: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueueOperationEntry {
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub timestamp: Option<String>,
    pub operation: String,
}

pub type ToolUseResultData = serde_json::Value;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_text_content() {
        let json = r#"{"type":"text","text":"hello world"}"#;
        let block: ContentBlock = serde_json::from_str(json).unwrap();
        assert_eq!(
            block,
            ContentBlock::Text {
                text: "hello world".to_string()
            }
        );
    }

    #[test]
    fn deserialize_thinking_content() {
        let json = r#"{"type":"thinking","thinking":"let me think","signature":"sig123"}"#;
        let block: ContentBlock = serde_json::from_str(json).unwrap();
        assert_eq!(
            block,
            ContentBlock::Thinking {
                thinking: "let me think".to_string(),
                signature: "sig123".to_string()
            }
        );
    }

    #[test]
    fn deserialize_tool_use_content() {
        let json = r#"{"type":"tool_use","id":"tu_1","name":"Read","input":{"file_path":"/tmp/test.txt"}}"#;
        let block: ContentBlock = serde_json::from_str(json).unwrap();
        assert_eq!(
            block,
            ContentBlock::ToolUse {
                id: "tu_1".to_string(),
                name: "Read".to_string(),
                input: serde_json::json!({"file_path": "/tmp/test.txt"})
            }
        );
    }

    #[test]
    fn deserialize_tool_result_content() {
        let json = r#"{"type":"tool_result","tool_use_id":"tu_1","content":"file contents here","is_error":false}"#;
        let block: ContentBlock = serde_json::from_str(json).unwrap();
        assert_eq!(
            block,
            ContentBlock::ToolResult {
                tool_use_id: "tu_1".to_string(),
                content: serde_json::Value::String("file contents here".to_string()),
                is_error: false
            }
        );
    }

    #[test]
    fn deserialize_user_entry() {
        let json = r#"{
            "type":"user",
            "uuid":"u1",
            "parentUuid":null,
            "timestamp":"2026-03-25T06:00:00.000Z",
            "message":{"role":"user","content":"hello"},
            "isMeta":false,
            "isSidechain":false,
            "userType":"external",
            "cwd":"/tmp",
            "sessionId":"s1",
            "version":"1.0",
            "gitBranch":"main"
        }"#;
        let entry: ChatHistoryEntry = serde_json::from_str(json).unwrap();
        match entry {
            ChatHistoryEntry::User(u) => {
                assert_eq!(u.uuid, "u1");
                assert_eq!(u.is_meta, Some(false));
            }
            _ => panic!("Expected UserEntry"),
        }
    }

    #[test]
    fn deserialize_assistant_entry() {
        let json = r#"{
            "type":"assistant",
            "uuid":"a1",
            "timestamp":"2026-03-25T06:00:01.000Z",
            "message":{
                "role":"assistant",
                "id":"msg_1",
                "type":"message",
                "model":"claude-3-opus",
                "content":[{"type":"text","text":"Hi!"}],
                "stop_reason":"end_turn",
                "stop_sequence":null,
                "usage":{"input_tokens":100,"output_tokens":10}
            },
            "requestId":"r1",
            "isSidechain":false,
            "userType":"external",
            "cwd":"/tmp",
            "sessionId":"s1",
            "version":"1.0",
            "gitBranch":"main"
        }"#;
        let entry: ChatHistoryEntry = serde_json::from_str(json).unwrap();
        match entry {
            ChatHistoryEntry::Assistant(a) => {
                assert_eq!(a.uuid, "a1");
                assert_eq!(a.message.model, "claude-3-opus");
            }
            _ => panic!("Expected AssistantEntry"),
        }
    }

    #[test]
    fn deserialize_user_entry_with_compact_summary() {
        let json = r#"{
            "type":"user",
            "uuid":"u1",
            "timestamp":"2026-03-25T06:00:00.000Z",
            "message":{"role":"user","content":"compacted summary"},
            "isMeta":true,
            "isCompactSummary":true,
            "isSidechain":false
        }"#;
        let entry: ChatHistoryEntry = serde_json::from_str(json).unwrap();
        match entry {
            ChatHistoryEntry::User(u) => {
                assert_eq!(u.is_compact_summary, Some(true));
                assert_eq!(u.is_meta, Some(true));
            }
            _ => panic!("Expected UserEntry"),
        }
    }

    #[test]
    fn deserialize_meta_user_without_compact_summary() {
        let json = r#"{
            "type":"user",
            "uuid":"u2",
            "timestamp":"2026-03-25T06:00:00.000Z",
            "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","content":"result"}]},
            "isMeta":true,
            "isSidechain":false
        }"#;
        let entry: ChatHistoryEntry = serde_json::from_str(json).unwrap();
        match entry {
            ChatHistoryEntry::User(u) => {
                assert_eq!(u.is_compact_summary, None);
                assert_eq!(u.is_meta, Some(true));
            }
            _ => panic!("Expected UserEntry"),
        }
    }

    #[test]
    fn deserialize_fixture_file() {
        let content =
            std::fs::read_to_string("fixtures/sample-session.jsonl").unwrap();
        let count = content
            .lines()
            .filter(|l| !l.is_empty())
            .filter_map(|l| serde_json::from_str::<ChatHistoryEntry>(l).ok())
            .count();
        assert!(count > 0, "Should parse at least one entry from fixture");
    }

    #[test]
    fn deserialize_fixture_counts_by_type() {
        let content =
            std::fs::read_to_string("fixtures/sample-session.jsonl").unwrap();
        let mut user_count = 0u32;
        let mut assistant_count = 0u32;
        let mut system_count = 0u32;
        let mut other_count = 0u32;

        for line in content.lines().filter(|l| !l.is_empty()) {
            if let Ok(entry) = serde_json::from_str::<ChatHistoryEntry>(line) {
                match entry {
                    ChatHistoryEntry::User(_) => user_count += 1,
                    ChatHistoryEntry::Assistant(_) => assistant_count += 1,
                    ChatHistoryEntry::System(_) => system_count += 1,
                    _ => other_count += 1,
                }
            }
        }

        assert!(user_count > 0, "Should have user entries");
        assert!(assistant_count > 0, "Should have assistant entries");
        eprintln!(
            "Fixture stats: user={}, assistant={}, system={}, other={}",
            user_count, assistant_count, system_count, other_count
        );
    }
}
