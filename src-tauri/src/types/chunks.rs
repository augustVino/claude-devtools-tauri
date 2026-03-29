use serde::{Deserialize, Serialize};

use super::messages::{ParsedMessage, SemanticStep, SemanticStepGroup, ToolCall, ToolResult};
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
    #[serde(rename = "semanticSteps", default, skip_serializing_if = "Vec::is_empty")]
    pub semantic_steps: Vec<SemanticStep>,
    #[serde(rename = "semanticStepGroups", default, skip_serializing_if = "Vec::is_empty")]
    pub semantic_step_groups: Vec<SemanticStepGroup>,
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
    #[serde(rename = "toolCall")]
    pub tool_call: ToolCall,
    pub result: Option<ToolResult>,
    #[serde(rename = "startTime")]
    pub start_time: String,
    #[serde(rename = "endTime")]
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

// =============================================================================
// SubagentResolver Process → Chunk Process conversion
// =============================================================================

/// Convert subagent_resolver::Process → chunks::Process.
impl From<crate::discovery::subagent_resolver::Process> for Process {
    fn from(p: crate::discovery::subagent_resolver::Process) -> Self {
        Self {
            id: p.id,
            file_path: p.file_path,
            description: p.description,
            subagent_type: p.subagent_type,
            messages: vec![],
            start_time: p.start_time_ms,
            end_time: p.end_time_ms,
            duration_ms: p.duration_ms,
            metrics: p.metrics,
            is_parallel: p.is_parallel,
            parent_task_id: p.task_id,
            is_ongoing: Some(p.is_ongoing),
            main_session_impact: None,
            team: p.team,
        }
    }
}

// =============================================================================
// Task Execution
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskExecution {
    pub task_id: String,
    #[serde(rename = "toolCallId")]
    pub tool_call_id: String,
    pub input: serde_json::Value,
    pub subagent: Process,
    #[serde(rename = "toolResult")]
    pub tool_result: ParsedMessage,
    #[serde(rename = "taskCallTimestamp")]
    pub task_call_timestamp: f64,
    #[serde(rename = "resultTimestamp")]
    pub result_timestamp: f64,
    #[serde(rename = "durationMs")]
    pub duration_ms: u64,
}

// =============================================================================
// Conversation Group
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationGroup {
    pub id: String,
    pub r#type: String,
    #[serde(rename = "userMessage")]
    pub user_message: ParsedMessage,
    #[serde(rename = "aiResponses")]
    pub ai_responses: Vec<ParsedMessage>,
    pub processes: Vec<Process>,
    #[serde(rename = "toolExecutions")]
    pub tool_executions: Vec<ToolExecution>,
    #[serde(rename = "taskExecutions")]
    pub task_executions: Vec<TaskExecution>,
    #[serde(rename = "startTime")]
    pub start_time: f64,
    #[serde(rename = "endTime")]
    pub end_time: f64,
    #[serde(rename = "durationMs")]
    pub duration_ms: u64,
    pub metrics: SessionMetrics,
}
