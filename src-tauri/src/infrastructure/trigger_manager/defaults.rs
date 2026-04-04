//! 默认触发器定义。

use crate::types::config::{
    NotificationTrigger, TriggerContentType, TriggerMode, TriggerTokenType,
};

/// 返回三个默认的内置通知触发器。
pub fn default_triggers() -> Vec<NotificationTrigger> {
    vec![
        NotificationTrigger {
            id: "builtin-bash-command".to_string(),
            name: ".env File Access Alert".to_string(),
            enabled: false,
            content_type: TriggerContentType::ToolUse,
            mode: TriggerMode::ContentMatch,
            match_pattern: Some("/.env".to_string()),
            is_builtin: Some(true),
            color: Some("red".to_string()),
            tool_name: None,
            ignore_patterns: None,
            require_error: None,
            match_field: None,
            token_threshold: None,
            token_type: None,
            repository_ids: None,
        },
        NotificationTrigger {
            id: "builtin-tool-result-error".to_string(),
            name: "Tool Result Error".to_string(),
            enabled: false,
            content_type: TriggerContentType::ToolResult,
            mode: TriggerMode::ErrorStatus,
            require_error: Some(true),
            ignore_patterns: Some(vec![
                r"The user doesn't want to proceed with this tool use\.".to_string(),
                r"\[Request interrupted by user for tool use\]".to_string(),
            ]),
            is_builtin: Some(true),
            color: Some("orange".to_string()),
            tool_name: None,
            match_field: None,
            match_pattern: None,
            token_threshold: None,
            token_type: None,
            repository_ids: None,
        },
        NotificationTrigger {
            id: "builtin-high-token-usage".to_string(),
            name: "High Token Usage".to_string(),
            enabled: false,
            content_type: TriggerContentType::ToolResult,
            mode: TriggerMode::TokenThreshold,
            token_threshold: Some(8000),
            token_type: Some(TriggerTokenType::Total),
            color: Some("yellow".to_string()),
            is_builtin: Some(true),
            tool_name: None,
            ignore_patterns: None,
            require_error: None,
            match_field: None,
            match_pattern: None,
            repository_ids: None,
        },
    ]
}
