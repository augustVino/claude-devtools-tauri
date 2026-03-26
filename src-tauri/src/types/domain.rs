use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::jsonl::UsageMetadata;

pub type TokenUsage = UsageMetadata;

// =============================================================================
// Enums
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    #[default]
    User,
    Assistant,
    System,
    Summary,
    #[serde(rename = "file-history-snapshot")]
    FileHistorySnapshot,
    #[serde(rename = "queue-operation")]
    QueueOperation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MessageCategory {
    User,
    System,
    HardNoise,
    Ai,
    Compact,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SessionMetadataLevel {
    Light,
    Deep,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WorktreeSource {
    #[serde(rename = "vibe-kanban")]
    VibeKanban,
    #[serde(rename = "conductor")]
    Conductor,
    #[serde(rename = "auto-claude")]
    AutoClaude,
    #[serde(rename = "21st")]
    TwentyFirst,
    #[serde(rename = "claude-desktop")]
    ClaudeDesktop,
    #[serde(rename = "ccswitch")]
    Ccswitch,
    #[serde(rename = "git")]
    Git,
    #[serde(other)]
    Unknown,
}

// =============================================================================
// Project & Session
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Project {
    pub id: String,
    pub path: String,
    pub name: String,
    pub sessions: Vec<String>,
    #[serde(rename = "createdAt")]
    pub created_at: u64,
    #[serde(rename = "mostRecentSession", skip_serializing_if = "Option::is_none")]
    pub most_recent_session: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PhaseTokenBreakdown {
    #[serde(rename = "phaseNumber")]
    pub phase_number: u32,
    pub contribution: u64,
    #[serde(rename = "peakTokens")]
    pub peak_tokens: u64,
    #[serde(rename = "postCompaction", skip_serializing_if = "Option::is_none")]
    pub post_compaction: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Session {
    pub id: String,
    #[serde(rename = "projectId")]
    pub project_id: String,
    #[serde(rename = "projectPath")]
    pub project_path: String,
    #[serde(rename = "createdAt")]
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todo_data: Option<serde_json::Value>,
    #[serde(rename = "firstMessage", skip_serializing_if = "Option::is_none")]
    pub first_message: Option<String>,
    #[serde(rename = "messageTimestamp", skip_serializing_if = "Option::is_none")]
    pub message_timestamp: Option<String>,
    #[serde(rename = "hasSubagents")]
    pub has_subagents: bool,
    #[serde(rename = "messageCount")]
    pub message_count: u32,
    #[serde(rename = "isOngoing", skip_serializing_if = "Option::is_none")]
    pub is_ongoing: Option<bool>,
    #[serde(rename = "gitBranch", skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    #[serde(rename = "metadataLevel", skip_serializing_if = "Option::is_none")]
    pub metadata_level: Option<SessionMetadataLevel>,
    #[serde(rename = "contextConsumption", skip_serializing_if = "Option::is_none")]
    pub context_consumption: Option<u64>,
    #[serde(rename = "compactionCount", skip_serializing_if = "Option::is_none")]
    pub compaction_count: Option<u32>,
    #[serde(rename = "phaseBreakdown", skip_serializing_if = "Option::is_none")]
    pub phase_breakdown: Option<Vec<PhaseTokenBreakdown>>,
}

// =============================================================================
// Metrics
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SessionMetrics {
    #[serde(rename = "durationMs")]
    pub duration_ms: u64,
    #[serde(rename = "totalTokens")]
    pub total_tokens: u64,
    #[serde(rename = "inputTokens")]
    pub input_tokens: u64,
    #[serde(rename = "outputTokens")]
    pub output_tokens: u64,
    #[serde(rename = "cacheReadTokens", skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(rename = "cacheCreationTokens", skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u64>,
    #[serde(rename = "messageCount")]
    pub message_count: u32,
    #[serde(rename = "costUsd", skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

// =============================================================================
// Repository & Worktree
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepositoryIdentity {
    pub id: String,
    #[serde(rename = "mainGitDir")]
    pub main_git_dir: String,
    pub name: String,
    #[serde(rename = "remoteUrl", skip_serializing_if = "Option::is_none")]
    pub remote_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Worktree {
    pub id: String,
    pub path: String,
    pub name: String,
    pub sessions: Vec<String>,
    #[serde(rename = "gitBranch", skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    #[serde(rename = "isMainWorktree")]
    pub is_main_worktree: bool,
    pub source: WorktreeSource,
    #[serde(rename = "createdAt")]
    pub created_at: u64,
    #[serde(rename = "mostRecentSession", skip_serializing_if = "Option::is_none")]
    pub most_recent_session: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepositoryGroup {
    pub id: String,
    pub name: String,
    pub identity: Option<RepositoryIdentity>,
    pub worktrees: Vec<Worktree>,
    #[serde(rename = "mostRecentSession", skip_serializing_if = "Option::is_none")]
    pub most_recent_session: Option<u64>,
    #[serde(rename = "totalSessions")]
    pub total_sessions: u32,
}

// =============================================================================
// Search
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResult {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "projectId")]
    pub project_id: String,
    #[serde(rename = "sessionTitle")]
    pub session_title: String,
    #[serde(rename = "matchedText")]
    pub matched_text: String,
    pub context: String,
    #[serde(rename = "messageType")]
    pub message_type: String,
    pub timestamp: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(rename = "itemType", skip_serializing_if = "Option::is_none")]
    pub item_type: Option<String>,
    #[serde(rename = "matchIndexInItem", skip_serializing_if = "Option::is_none")]
    pub match_index_in_item: Option<u32>,
    #[serde(rename = "matchStartOffset", skip_serializing_if = "Option::is_none")]
    pub match_start_offset: Option<u32>,
    #[serde(rename = "messageUuid", skip_serializing_if = "Option::is_none")]
    pub message_uuid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchSessionsResult {
    pub results: Vec<SearchResult>,
    #[serde(rename = "totalMatches")]
    pub total_matches: u32,
    #[serde(rename = "sessionsSearched")]
    pub sessions_searched: u32,
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_partial: Option<bool>,
}

// =============================================================================
// Pagination
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionCursor {
    pub timestamp: u64,
    #[serde(rename = "sessionId")]
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PaginatedSessionsResult {
    pub sessions: Vec<Session>,
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(rename = "hasMore")]
    pub has_more: bool,
    #[serde(rename = "totalCount")]
    pub total_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SessionsPaginationOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_total_count: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefilter_all: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_level: Option<SessionMetadataLevel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SessionsByIdsOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_level: Option<SessionMetadataLevel>,
}

// =============================================================================
// File Change Event
// =============================================================================

/// File change event type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FileChangeType {
    Add,
    Change,
    Unlink,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileChangeEvent {
    #[serde(rename = "type")]
    pub event_type: FileChangeType,
    pub path: String,
    #[serde(rename = "projectId", skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(rename = "isSubagent")]
    pub is_subagent: bool,
}

// =============================================================================
// IPC Result
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IpcResult<T> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
