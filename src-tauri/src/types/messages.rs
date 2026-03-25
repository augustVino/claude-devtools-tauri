use serde::{Deserialize, Serialize};

use super::jsonl::{ContentBlock, ToolUseResultData, UsageMetadata};
use crate::types::domain::{MessageType, SessionMetrics};

// =============================================================================
// Tool Call & Result
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
    #[serde(rename = "isTask")]
    pub is_task: bool,
    #[serde(rename = "taskDescription", skip_serializing_if = "Option::is_none")]
    pub task_description: Option<String>,
    #[serde(rename = "taskSubagentType", skip_serializing_if = "Option::is_none")]
    pub task_subagent_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolResult {
    #[serde(rename = "toolUseId")]
    pub tool_use_id: String,
    pub content: serde_json::Value,
    #[serde(rename = "isError")]
    pub is_error: bool,
}

// =============================================================================
// ParsedMessage
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParsedMessage {
    pub uuid: String,
    #[serde(rename = "parentUuid", skip_serializing_if = "Option::is_none")]
    pub parent_uuid: Option<String>,
    #[serde(rename = "type")]
    pub message_type: MessageType,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, deserialize_with = "deserialize_message_content")]
    pub content: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(rename = "gitBranch", skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    #[serde(rename = "agentId", skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(rename = "isSidechain", default)]
    pub is_sidechain: bool,
    #[serde(rename = "isMeta", default)]
    pub is_meta: bool,
    #[serde(rename = "userType", skip_serializing_if = "Option::is_none")]
    pub user_type: Option<String>,
    #[serde(rename = "toolCalls", default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(rename = "toolResults", default)]
    pub tool_results: Vec<ToolResult>,
    #[serde(rename = "sourceToolUseID", skip_serializing_if = "Option::is_none")]
    pub source_tool_use_id: Option<String>,
    #[serde(rename = "sourceToolAssistantUUID", skip_serializing_if = "Option::is_none")]
    pub source_tool_assistant_uuid: Option<String>,
    #[serde(rename = "toolUseResult", skip_serializing_if = "Option::is_none")]
    pub tool_use_result: Option<ToolUseResultData>,
    #[serde(rename = "isCompactSummary", skip_serializing_if = "Option::is_none")]
    pub is_compact_summary: Option<bool>,
    #[serde(rename = "requestId", skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

fn deserialize_message_content<'de, D>(deserializer: D) -> Result<serde_json::Value, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde_json::Value::deserialize(deserializer)
}

// =============================================================================
// Display Items
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SemanticStepType {
    Thinking,
    ToolCall,
    ToolResult,
    Subagent,
    Output,
    Interruption,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SemanticStep {
    pub id: String,
    #[serde(rename = "type")]
    pub step_type: SemanticStepType,
    #[serde(rename = "startTime")]
    pub start_time: String,
    #[serde(rename = "endTime", skip_serializing_if = "Option::is_none")]
    pub end_time: Option<String>,
    #[serde(rename = "durationMs")]
    pub duration_ms: u64,
    pub content: SemanticStepContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<StepTokens>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_parallel: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    pub context: String,
    #[serde(rename = "agentId", skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(rename = "sourceMessageId", skip_serializing_if = "Option::is_none")]
    pub source_message_id: Option<String>,
    #[serde(rename = "effectiveEndTime", skip_serializing_if = "Option::is_none")]
    pub effective_end_time: Option<String>,
    #[serde(rename = "effectiveDurationMs", skip_serializing_if = "Option::is_none")]
    pub effective_duration_ms: Option<u64>,
    #[serde(rename = "isGapFilled", skip_serializing_if = "Option::is_none")]
    pub is_gap_filled: Option<bool>,
    #[serde(rename = "contextTokens", skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u64>,
    #[serde(rename = "accumulatedContext", skip_serializing_if = "Option::is_none")]
    pub accumulated_context: Option<u64>,
    #[serde(rename = "tokenBreakdown", skip_serializing_if = "Option::is_none")]
    pub token_breakdown: Option<TokenBreakdown>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct SemanticStepContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_output: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent_description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_result: Option<ToolUseResultData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slash_command: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepTokens {
    pub input: u64,
    pub output: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenBreakdown {
    pub input: u64,
    pub output: u64,
    #[serde(rename = "cacheRead")]
    pub cache_read: u64,
    #[serde(rename = "cacheCreation")]
    pub cache_creation: u64,
}
