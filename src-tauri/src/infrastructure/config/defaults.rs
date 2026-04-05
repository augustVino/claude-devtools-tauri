//! 默认配置构造函数和常量。

use crate::types::config::{AppConfig, DisplayConfig, GeneralConfig, NotificationConfig, SessionConfig};

/// 配置文件名
pub(crate) const CONFIG_FILENAME: &str = "claude-devtools-config.json";

/// 默认忽略的正则表达式列表（匹配用户拒绝工具使用的消息）
pub(crate) const DEFAULT_IGNORED_REGEX: &[&str] =
    &[r"The user doesn't want to proceed with this tool use\."];

/// 构造默认应用配置
pub(crate) fn default_app_config() -> AppConfig {
    AppConfig {
        notifications: NotificationConfig {
            enabled: true,
            sound_enabled: true,
            ignored_regex: DEFAULT_IGNORED_REGEX.iter().map(|s| s.to_string()).collect(),
            ignored_repositories: vec![],
            snoozed_until: None,
            snooze_minutes: 30,
            include_subagent_errors: true,
            triggers: vec![],
        },
        general: GeneralConfig {
            launch_at_login: false,
            show_dock_icon: true,
            theme: "dark".to_string(),
            default_tab: "dashboard".to_string(),
            claude_root_path: None,
            auto_expand_ai_groups: false,
            use_native_title_bar: false,
        },
        display: DisplayConfig {
            show_timestamps: true,
            compact_mode: false,
            syntax_highlighting: true,
        },
        sessions: SessionConfig {
            pinned_sessions: std::collections::HashMap::new(),
            hidden_sessions: std::collections::HashMap::new(),
        },
        ssh: Some(crate::types::config::SshConfig {
            last_connection: None,
            auto_reconnect: false,
            profiles: vec![],
            last_active_context_id: String::new(),
        }),
        http_server: None,
    }
}

/// 将默认配置序列化为 JSON Value
pub(crate) fn default_config_json() -> serde_json::Value {
    serde_json::to_value(default_app_config()).expect("default config must serialize")
}

/// 获取当前时间的毫秒级 UNIX 时间戳
pub(crate) fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
