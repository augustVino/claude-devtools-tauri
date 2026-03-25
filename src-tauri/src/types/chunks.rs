use serde::{Deserialize, Serialize};

use super::messages::{ParsedMessage, SemanticStep, ToolCall, ToolResult};
use crate::types::domain::{Session, SessionMetrics};

// =============================================================================
// Chunk Types
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "chunkType", rename_all = "lowercase")]
pub enum Chunk {
    User(UserChunk),
    Ai(AiChunk),
    System(SystemChunk),
    Compact(CompactChunk),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserChunk {
    pub id: String,
    #[serde(rename = "startTime")]
    pub start_time: u64,
    #[serde(rename = "endTime")]
    pub end_time: u64,
    #[serde(rename = "durationMs")]
    pub duration_ms: u64,
    pub metrics: SessionMetrics,
    #[serde(rename = "userMessage")]
    pub user_message: ParsedMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AiChunk {
    pub id: String,
    #[serde(rename = "startTime")]
    pub start_time: u64,
    #[serde(rename = "endTime")]
    pub end_time: u64,
    #[serde(rename = "durationMs")]
    pub duration_ms: u64,
    pub metrics: SessionMetrics,
    pub responses: Vec<ParsedMessage>,
    #[serde(default)]
    pub processes: Vec<Process>,
    #[serde(rename = "sidechainMessages", default)]
    pub sidechain_messages: Vec<ParsedMessage>,
    #[serde(rename = "toolExecutions", default)]
    pub tool_executions: Vec<ToolExecution>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SystemChunk {
    pub id: String,
    #[serde(rename = "startTime")]
    pub start_time: u64,
    #[serde(rename = "endTime")]
    pub end_time: u64,
    #[serde(rename = "durationMs")]
    pub duration_ms: u64,
    pub metrics: SessionMetrics,
    pub message: ParsedMessage,
    #[serde(rename = "commandOutput")]
    pub command_output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompactChunk {
    pub id: String,
    #[serde(rename = "startTime")]
    pub start_time: u64,
    #[serde(rename = "endTime")]
    pub end_time: u64,
    #[serde(rename = "durationMs")]
    pub duration_ms: u64,
    pub metrics: SessionMetrics,
    pub message: ParsedMessage,
}

// =============================================================================
// Process (Subagent)
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Process {
    pub id: String,
    #[serde(rename = "filePath")]
    pub file_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "subagentType", skip_serializing_if = "Option::is_none")]
    pub subagent_type: Option<String>,
    pub messages: Vec<ParsedMessage>,
    #[serde(rename = "startTime")]
    pub start_time: u64,
    #[serde(rename = "endTime")]
    pub end_time: u64,
    #[serde(rename = "durationMs")]
    pub duration_ms: u64,
    pub metrics: SessionMetrics,
    #[serde(rename = "isParallel")]
    pub is_parallel: bool,
    #[serde(rename = "parentTaskId", skip_serializing_if = "Option::is_none")]
    pub parent_task_id: Option<String>,
    #[serde(rename = "isOngoing", skip_serializing_if = "Option::is_none")]
    pub is_ongoing: Option<bool>,
    #[serde(rename = "mainSessionImpact", skip_serializing_if = "Option::is_none")]
    pub main_session_impact: Option<SessionImpact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team: Option<TeamInfo>,
}

// =============================================================================
// Session Detail
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionDetail {
    pub session: Session,
    pub messages: Vec<ParsedMessage>,
    pub chunks: Vec<Chunk>,
    pub processes: Vec<Process>,
    pub metrics: SessionMetrics,
}

// =============================================================================
// Tool Execution
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolExecution {
    pub tool_call: ToolCall,
    pub result: Option<ToolResult>,
    #[serde(rename = "startTime")]
    pub start_time: String,
    pub end_time: Option<String>,
    #[serde(rename = "durationMs")]
    pub duration_ms: Option<u64>,
}

// =============================================================================
// Session Impact
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionImpact {
    #[serde(rename = "callTokens")]
    pub call_tokens: u64,
    #[serde(rename = "resultTokens")]
    pub result_tokens: u64,
}

// =============================================================================
// Team Info
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TeamInfo {
    #[serde(rename = "teamName")]
    pub team_name: String,
    #[serde(rename = "memberName")]
    pub member_name: String,
    #[serde(rename = "memberColor")]
    pub member_color: String,
}
