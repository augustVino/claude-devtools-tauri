use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// =============================================================================
// Trigger Types
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TriggerMode {
    ErrorStatus,
    ContentMatch,
    TokenThreshold,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TriggerContentType {
    ToolResult,
    ToolUse,
    Thinking,
    Text,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TriggerTokenType {
    Input,
    Output,
    Total,
}

impl TriggerTokenType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::Total => "total",
        }
    }
}

// =============================================================================
// Notification Trigger
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NotificationTrigger {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    #[serde(rename = "contentType")]
    pub content_type: TriggerContentType,
    #[serde(rename = "toolName", skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(rename = "isBuiltin", skip_serializing_if = "Option::is_none")]
    pub is_builtin: Option<bool>,
    #[serde(
        rename = "ignorePatterns",
        alias = "ignore_patterns",
        skip_serializing_if = "Option::is_none"
    )]
    pub ignore_patterns: Option<Vec<String>>,
    pub mode: TriggerMode,
    #[serde(rename = "requireError", skip_serializing_if = "Option::is_none")]
    pub require_error: Option<bool>,
    #[serde(rename = "matchField", skip_serializing_if = "Option::is_none")]
    pub match_field: Option<String>,
    #[serde(rename = "matchPattern", skip_serializing_if = "Option::is_none")]
    pub match_pattern: Option<String>,
    #[serde(rename = "tokenThreshold", skip_serializing_if = "Option::is_none")]
    pub token_threshold: Option<u64>,
    #[serde(rename = "tokenType", skip_serializing_if = "Option::is_none")]
    pub token_type: Option<TriggerTokenType>,
    #[serde(rename = "repositoryIds", skip_serializing_if = "Option::is_none")]
    pub repository_ids: Option<Vec<String>>,
    pub color: Option<String>,
}

// =============================================================================
// Detected Error
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DetectedError {
    pub id: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "projectId")]
    pub project_id: String,
    #[serde(rename = "filePath")]
    pub file_path: String,
    pub source: String,
    pub message: String,
    pub timestamp: u64,
    #[serde(rename = "lineNumber", skip_serializing_if = "Option::is_none")]
    pub line_number: Option<u64>,
    #[serde(rename = "toolUseId", skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(rename = "subagentId", skip_serializing_if = "Option::is_none")]
    pub subagent_id: Option<String>,
    #[serde(rename = "triggerColor", skip_serializing_if = "Option::is_none")]
    pub trigger_color: Option<String>,
    #[serde(rename = "triggerId", skip_serializing_if = "Option::is_none")]
    pub trigger_id: Option<String>,
    #[serde(rename = "triggerName", skip_serializing_if = "Option::is_none")]
    pub trigger_name: Option<String>,
    pub context: ErrorContext,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorContext {
    #[serde(rename = "projectName")]
    pub project_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

// =============================================================================
// Trigger Test Result
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TriggerTestResult {
    #[serde(rename = "totalCount")]
    pub total_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    pub errors: Vec<TriggerTestError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TriggerTestError {
    pub id: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "projectId")]
    pub project_id: String,
    pub message: String,
    pub timestamp: u64,
    pub source: String,
    #[serde(rename = "toolUseId", skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(rename = "subagentId", skip_serializing_if = "Option::is_none")]
    pub subagent_id: Option<String>,
    #[serde(rename = "lineNumber", skip_serializing_if = "Option::is_none")]
    pub line_number: Option<u64>,
    pub context: ErrorContext,
}

// =============================================================================
// App Config
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppConfig {
    pub notifications: NotificationConfig,
    pub general: GeneralConfig,
    pub display: DisplayConfig,
    pub sessions: SessionConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh: Option<SshConfig>,
    #[serde(rename = "httpServer", skip_serializing_if = "Option::is_none")]
    pub http_server: Option<HttpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NotificationConfig {
    pub enabled: bool,
    #[serde(rename = "soundEnabled")]
    pub sound_enabled: bool,
    #[serde(rename = "ignoredRegex", default)]
    pub ignored_regex: Vec<String>,
    #[serde(rename = "ignoredRepositories")]
    pub ignored_repositories: Vec<String>,
    #[serde(rename = "snoozedUntil", skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<u64>,
    #[serde(rename = "snoozeMinutes")]
    pub snooze_minutes: u32,
    #[serde(rename = "includeSubagentErrors")]
    pub include_subagent_errors: bool,
    pub triggers: Vec<NotificationTrigger>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GeneralConfig {
    #[serde(rename = "launchAtLogin")]
    pub launch_at_login: bool,
    #[serde(rename = "showDockIcon")]
    pub show_dock_icon: bool,
    pub theme: String,
    #[serde(rename = "defaultTab")]
    pub default_tab: String,
    #[serde(rename = "claudeRootPath", skip_serializing_if = "Option::is_none")]
    pub claude_root_path: Option<String>,
    #[serde(rename = "autoExpandAIGroups")]
    pub auto_expand_ai_groups: bool,
    #[serde(rename = "useNativeTitleBar")]
    pub use_native_title_bar: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DisplayConfig {
    #[serde(rename = "showTimestamps")]
    pub show_timestamps: bool,
    #[serde(rename = "compactMode")]
    pub compact_mode: bool,
    #[serde(rename = "syntaxHighlighting")]
    pub syntax_highlighting: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SessionConfig {
    #[serde(rename = "pinnedSessions")]
    pub pinned_sessions: HashMap<String, Vec<PinnedSession>>,
    #[serde(rename = "hiddenSessions")]
    pub hidden_sessions: HashMap<String, Vec<HiddenSession>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PinnedSession {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "pinnedAt")]
    pub pinned_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HiddenSession {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "hiddenAt")]
    pub hidden_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SshConfig {
    #[serde(rename = "lastConnection", skip_serializing_if = "Option::is_none")]
    pub last_connection: Option<SshConnection>,
    #[serde(rename = "autoReconnect")]
    pub auto_reconnect: bool,
    pub profiles: Vec<SshProfile>,
    #[serde(rename = "lastActiveContextId")]
    pub last_active_context_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SshConnection {
    pub host: String,
    pub username: String,
    pub port: u16,
    #[serde(rename = "authMethod")]
    pub auth_method: String,
    #[serde(rename = "privateKeyPath", skip_serializing_if = "Option::is_none")]
    pub private_key_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SshProfile {
    pub id: String,
    pub name: String,
    pub host: String,
    pub username: String,
    pub port: u16,
    #[serde(rename = "authMethod")]
    pub auth_method: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HttpServerConfig {
    pub enabled: bool,
    pub port: u16,
}

// =============================================================================
// Stored Notification
// =============================================================================

/// Stored notification — extends DetectedError with read state and creation time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StoredNotification {
    #[serde(flatten)]
    pub error: DetectedError,
    #[serde(rename = "isRead")]
    pub is_read: bool,
    #[serde(rename = "createdAt")]
    pub created_at: u64,
}

// =============================================================================
// Notification Query Types
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GetNotificationsOptions {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GetNotificationsResult {
    pub notifications: Vec<StoredNotification>,
    pub total: usize,
    #[serde(rename = "totalCount")]
    pub total_count: usize,
    #[serde(rename = "unreadCount")]
    pub unread_count: usize,
    #[serde(rename = "hasMore")]
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NotificationCountResult {
    pub total: usize,
    #[serde(rename = "unreadCount")]
    pub unread_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NotificationStats {
    pub total: usize,
    pub unread: usize,
    #[serde(rename = "byProject")]
    pub by_project: HashMap<String, usize>,
    #[serde(rename = "bySource")]
    pub by_source: HashMap<String, usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TriggerValidationResult {
    pub valid: bool,
    pub errors: Vec<String>,
}

// =============================================================================
// Claude Root Info
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeRootInfo {
    #[serde(rename = "defaultPath")]
    pub default_path: String,
    #[serde(rename = "resolvedPath")]
    pub resolved_path: String,
    #[serde(rename = "customPath", skip_serializing_if = "Option::is_none")]
    pub custom_path: Option<String>,
}
