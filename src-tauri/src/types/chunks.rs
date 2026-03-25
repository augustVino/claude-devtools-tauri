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
    pub start_time: String,
    #[serde(rename = "endTime")]
    pub end_time: String,
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
    pub start_time: String,
    #[serde(rename = "endTime")]
    pub end_time: String,
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
    pub start_time: String,
    #[serde(rename = "endTime")]
    pub end_time: String,
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
    pub start_time: String,
    #[serde(rename = "endTime")]
    pub end_time: String,
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
    pub start_time: String,
    #[serde(rename = "endTime")]
    pub end_time: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionImpact {
    #[serde(rename = "callTokens")]
    pub call_tokens: u64,
    #[serde(rename = "resultTokens")]
    pub result_tokens: u64,
    #[serde(rename = "totalTokens")]
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TeamInfo {
    #[serde(rename = "teamName")]
    pub team_name: String,
    #[serde(rename = "memberName")]
    pub member_name: String,
    #[serde(rename = "memberColor")]
    pub member_color: String,
}

// =============================================================================
// Tool Execution
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolExecution {
    #[serde(rename = "toolCall")]
    pub tool_call: ToolCall,
    pub result: Option<ToolResult>,
    #[serde(rename = "startTime")]
    pub start_time: String,
    #[serde(rename = "endTime", skip_serializing_if = "Option::is_none")]
    pub end_time: Option<String>,
    #[serde(rename = "durationMs", skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskExecution {
    #[serde(rename = "taskCall")]
    pub task_call: ToolCall,
    #[serde(rename = "taskCallTimestamp")]
    pub task_call_timestamp: String,
    pub subagent: Process,
    #[serde(rename = "toolResult")]
    pub tool_result: ParsedMessage,
    #[serde(rename = "resultTimestamp")]
    pub result_timestamp: String,
    #[serde(rename = "durationMs")]
    pub duration_ms: u64,
}

// =============================================================================
// Conversation Group
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationGroup {
    pub id: String,
    #[serde(rename = "type")]
    pub group_type: String,
    #[serde(rename = "userMessage")]
    pub user_message: ParsedMessage,
    #[serde(rename = "aiResponses")]
    pub ai_responses: Vec<ParsedMessage>,
    #[serde(default)]
    pub processes: Vec<Process>,
    #[serde(rename = "toolExecutions", default)]
    pub tool_executions: Vec<ToolExecution>,
    #[serde(rename = "taskExecutions", default)]
    pub task_executions: Vec<TaskExecution>,
    #[serde(rename = "startTime")]
    pub start_time: String,
    #[serde(rename = "endTime")]
    pub end_time: String,
    #[serde(rename = "durationMs")]
    pub duration_ms: u64,
    pub metrics: SessionMetrics,
}

// =============================================================================
// Enhanced Chunks (with semantic steps)
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "chunkType", rename_all = "lowercase")]
pub enum EnhancedChunk {
    User(EnhancedUserChunk),
    Ai(EnhancedAiChunk),
    System(EnhancedSystemChunk),
    Compact(EnhancedCompactChunk),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnhancedUserChunk {
    pub id: String,
    #[serde(rename = "startTime")]
    pub start_time: String,
    #[serde(rename = "endTime")]
    pub end_time: String,
    #[serde(rename = "durationMs")]
    pub duration_ms: u64,
    pub metrics: SessionMetrics,
    #[serde(rename = "userMessage")]
    pub user_message: ParsedMessage,
    #[serde(rename = "rawMessages")]
    pub raw_messages: Vec<ParsedMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnhancedAiChunk {
    pub id: String,
    #[serde(rename = "startTime")]
    pub start_time: String,
    #[serde(rename = "endTime")]
    pub end_time: String,
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
    #[serde(rename = "semanticSteps")]
    pub semantic_steps: Vec<SemanticStep>,
    #[serde(rename = "semanticStepGroups", skip_serializing_if = "Option::is_none")]
    pub semantic_step_groups: Option<Vec<SemanticStepGroup>>,
    #[serde(rename = "rawMessages")]
    pub raw_messages: Vec<ParsedMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnhancedSystemChunk {
    pub id: String,
    #[serde(rename = "startTime")]
    pub start_time: String,
    #[serde(rename = "endTime")]
    pub end_time: String,
    #[serde(rename = "durationMs")]
    pub duration_ms: u64,
    pub metrics: SessionMetrics,
    pub message: ParsedMessage,
    #[serde(rename = "rawMessages")]
    pub raw_messages: Vec<ParsedMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnhancedCompactChunk {
    pub id: String,
    #[serde(rename = "startTime")]
    pub start_time: String,
    #[serde(rename = "endTime")]
    pub end_time: String,
    #[serde(rename = "durationMs")]
    pub duration_ms: u64,
    pub metrics: SessionMetrics,
    pub message: ParsedMessage,
    #[serde(rename = "rawMessages")]
    pub raw_messages: Vec<ParsedMessage>,
}

// =============================================================================
// Session Detail & Subagent Detail
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionDetail {
    pub session: super::domain::Session,
    pub messages: Vec<ParsedMessage>,
    pub chunks: Vec<Chunk>,
    pub processes: Vec<Process>,
    pub metrics: SessionMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubagentDetail {
    pub id: String,
    pub description: String,
    pub chunks: Vec<EnhancedChunk>,
    #[serde(rename = "semanticStepGroups", skip_serializing_if = "Option::is_none")]
    pub semantic_step_groups: Option<Vec<SemanticStepGroup>>,
    #[serde(rename = "startTime")]
    pub start_time: String,
    #[serde(rename = "endTime")]
    pub end_time: String,
    pub duration: u64,
    pub metrics: SubagentMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubagentMetrics {
    #[serde(rename = "inputTokens")]
    pub input_tokens: u64,
    #[serde(rename = "outputTokens")]
    pub output_tokens: u64,
    #[serde(rename = "thinkingTokens")]
    pub thinking_tokens: u64,
    #[serde(rename = "messageCount")]
    pub message_count: u32,
}

// =============================================================================
// Semantic Step Group
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SemanticStepGroup {
    pub id: String,
    pub label: String,
    pub steps: Vec<SemanticStep>,
    #[serde(rename = "isGrouped")]
    pub is_grouped: bool,
    #[serde(rename = "sourceMessageId", skip_serializing_if = "Option::is_none")]
    pub source_message_id: Option<String>,
    #[serde(rename = "startTime")]
    pub start_time: String,
    #[serde(rename = "endTime")]
    pub end_time: String,
    #[serde(rename = "totalDuration")]
    pub total_duration: u64,
}
