//! 配置管理器 — 管理应用配置的加载、合并、分区更新和持久化。
//!
//! 配置文件路径: `~/.claude/claude-devtools-config.json`
//!
//! 核心特性:
//! - 加载时与默认值进行深度合并，新增字段自动填充
//! - 支持按分区更新 (notifications / general / display / sessions)
//! - 会话的置顶/隐藏/批量操作
//! - 通知的正则忽略列表和仓库忽略列表管理
//! - 通知暂停 (snooze) 和恢复
//! - 通知触发器的 CRUD 操作

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use crate::types::*;
use log::{error, info};
use serde_json;
use tokio::fs;

/// 配置文件名
const CONFIG_FILENAME: &str = "claude-devtools-config.json";

/// 默认忽略的正则表达式列表（匹配用户拒绝工具使用的消息）
const DEFAULT_IGNORED_REGEX: &[&str] =
    &[r"The user doesn't want to proceed with this tool use\."];

/// 构造默认应用配置
fn default_app_config() -> AppConfig {
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
            pinned_sessions: HashMap::new(),
            hidden_sessions: HashMap::new(),
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
fn default_config_json() -> serde_json::Value {
    serde_json::to_value(default_app_config()).expect("default config must serialize")
}

/// 获取当前时间的毫秒级 UNIX 时间戳
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// 应用配置管理器
///
/// 负责从磁盘加载配置、深度合并默认值、分区更新和持久化。
/// 内部使用 `RwLock` 保证线程安全的读写访问。
pub struct ConfigManager {
    /// 当前配置（受读写锁保护）
    config: RwLock<AppConfig>,
    /// 配置文件路径
    config_path: PathBuf,
}

impl ConfigManager {
    /// 使用默认路径 (`~/.claude/claude-devtools-config.json`) 创建配置管理器
    pub fn new() -> Self {
        let config_path = dirs::home_dir()
            .expect("home directory must exist")
            .join(".claude")
            .join(CONFIG_FILENAME);
        Self {
            config: RwLock::new(default_app_config()),
            config_path,
        }
    }

    /// 使用自定义路径创建配置管理器（主要用于测试）
    pub fn with_path(path: PathBuf) -> Self {
        Self {
            config: RwLock::new(default_app_config()),
            config_path: path,
        }
    }

    /// 初始化：从磁盘加载配置并与默认值深度合并。
    /// 如果配置文件不存在则使用默认值并写入磁盘。
    /// 同时将内置触发器合并到配置中（与 Electron 的 TriggerManager.mergeTriggers 一致）。
    pub async fn initialize(&self) -> Result<(), String> {
        let file_existed = self.config_path.exists();
        let mut loaded = self.load_config().await?;
        // 合并内置触发器：保留用户修改过的内置触发器，添加缺失的，移除废弃的
        loaded.notifications.triggers = crate::infrastructure::trigger_manager::TriggerManager::merge_triggers(
            loaded.notifications.triggers,
            &crate::infrastructure::trigger_manager::default_triggers(),
        );
        {
            let mut config = self
                .config
                .write()
                .map_err(|e| format!("failed to acquire write lock: {e}"))?;
            *config = loaded;
        } // 释放写锁后再持久化
        // 首次启动时将默认配置写入磁盘
        if !file_existed {
            self.persist()?;
            info!("Created default config file at {:?}", self.config_path);
        }
        Ok(())
    }

    /// 返回配置文件的路径
    pub fn get_config_path(&self) -> std::path::PathBuf {
        self.config_path.clone()
    }

    /// 获取当前配置的完整副本。
    /// 获取锁失败时返回默认配置并记录错误日志。
    pub fn get_config(&self) -> AppConfig {
        self.config
            .read()
            .map(|c| c.clone())
            .unwrap_or_else(|e| {
                error!("Failed to read config lock, returning defaults: {e}");
                default_app_config()
            })
    }

    /// 分区更新配置。
    ///
    /// 支持六个分区：`notifications`、`general`、`display`、`sessions`、`ssh`、`httpServer`。
    /// 更新前会对 payload 进行字段级校验（与 Electron 端对齐），包括类型、枚举、范围和路径检查。
    /// 更新后自动与默认值合并，确保新增字段有默认值。
    /// 更新完成后自动持久化到磁盘。
    pub fn update_config(
        &self,
        section: &str,
        mut data: serde_json::Value,
    ) -> Result<AppConfig, String> {
        let merged: AppConfig = {
            let mut config = self
                .config
                .write()
                .map_err(|e| format!("failed to acquire write lock: {e}"))?;

            let current_json = serde_json::to_value(&*config)
                .map_err(|e| format!("failed to serialize current config: {e}"))?;

            let valid_sections = ["notifications", "general", "display", "sessions", "ssh", "httpServer"];
            if !valid_sections.contains(&section) {
                return Err(format!("unknown config section: {section}"));
            }

            // Unified payload validation (aligned with Electron's validateConfigUpdatePayload)
            validate_update_payload(section, &data)?;

            // Normalize claudeRootPath (aligned with Electron's normalizeConfiguredClaudeRootPath)
            if section == "general" {
                if let Some(obj) = data.as_object_mut() {
                    if let Some(v) = obj.get_mut("claudeRootPath") {
                        if let Some(s) = v.as_str() {
                            let trimmed = s.trim();
                            if !trimmed.is_empty() {
                                *v = serde_json::Value::String(normalize_claude_root_path(trimmed));
                            }
                        }
                    }
                }
            }

            let updated = update_section(&current_json, section, &data);
            let merged: AppConfig = merge_with_defaults(&updated)?;
            *config = merged.clone();
            merged
        }; // 写锁在此处释放

        self.persist()?; // 持久化可以安全地获取读锁

        // Sync claude_root_path override when general section is updated
        if section == "general" {
            crate::utils::set_claude_root_override(merged.general.claude_root_path.clone());
        }

        Ok(merged)
    }

    /// 添加一个正则表达式到忽略列表。
    /// 会校验正则语法和去重。
    pub fn add_ignore_regex(&self, pattern: String) -> Result<AppConfig, String> {
        let trimmed = pattern.trim().to_string();
        if trimmed.is_empty() {
            return Err("pattern must not be empty".to_string());
        }
        if let Err(e) = regex::Regex::new(&trimmed) {
            return Err(format!("invalid regex pattern: {e}"));
        }

        let mut config = self
            .config
            .write()
            .map_err(|e| format!("failed to acquire write lock: {e}"))?;

        if config.notifications.ignored_regex.contains(&trimmed) {
            return Err("pattern already exists".to_string());
        }
        config.notifications.ignored_regex.push(trimmed);
        drop(config);
        self.persist()?;
        Ok(self.get_config())
    }

    /// 从忽略列表中移除指定的正则表达式
    pub fn remove_ignore_regex(&self, pattern: String) -> AppConfig {
        if let Ok(mut config) = self.config.write() {
            config.notifications.ignored_regex.retain(|p| p != &pattern);
            drop(config);
            let _ = self.persist();
        }
        self.get_config()
    }

    /// 添加一个仓库 ID 到忽略列表
    pub fn add_ignore_repository(&self, repo_id: String) -> AppConfig {
        let trimmed = repo_id.trim().to_string();
        if trimmed.is_empty() {
            return self.get_config();
        }
        if let Ok(mut config) = self.config.write() {
            if !config.notifications.ignored_repositories.contains(&trimmed) {
                config.notifications.ignored_repositories.push(trimmed);
                drop(config);
                let _ = self.persist();
            }
        }
        self.get_config()
    }

    /// 从忽略列表中移除指定的仓库 ID
    pub fn remove_ignore_repository(&self, repo_id: String) -> AppConfig {
        if let Ok(mut config) = self.config.write() {
            config.notifications.ignored_repositories.retain(|id| id != &repo_id);
            drop(config);
            let _ = self.persist();
        }
        self.get_config()
    }

    /// 暂停通知指定分钟数
    pub fn snooze(&self, minutes: u32) -> AppConfig {
        let snoozed_until = now_millis() + (minutes as u64) * 60 * 1000;
        if let Ok(mut config) = self.config.write() {
            config.notifications.snoozed_until = Some(snoozed_until);
            drop(config);
            let _ = self.persist();
            info!("Notifications snoozed for {minutes} minutes");
        }
        self.get_config()
    }

    /// Snooze notifications until midnight tomorrow (local time).
    ///
    /// This is the backend handler for the "Until tomorrow" UI option (sent as minutes = -1).
    pub fn snooze_until_tomorrow(&self) -> AppConfig {
        let tomorrow = chrono::Local::now().date_naive() + chrono::Duration::days(1);
        let tomorrow_midnight = tomorrow
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_local_timezone(chrono::Local)
            .single()
            .unwrap_or_else(|| {
                // Fallback for ambiguous DST: use noon tomorrow (close enough for "until tomorrow")
                tomorrow
                    .and_hms_opt(12, 0, 0)
                    .unwrap()
                    .and_local_timezone(chrono::Local)
                    .single()
                    .expect("noon should never be ambiguous")
            });

        let snoozed_until = tomorrow_midnight.timestamp_millis() as u64;

        if let Ok(mut config) = self.config.write() {
            config.notifications.snoozed_until = Some(snoozed_until);
            drop(config);
            let _ = self.persist();
            info!("Notifications snoozed until tomorrow midnight");
        }
        self.get_config()
    }

    /// 清除通知暂停状态，恢复通知
    pub fn clear_snooze(&self) -> AppConfig {
        if let Ok(mut config) = self.config.write() {
            config.notifications.snoozed_until = None;
            drop(config);
            let _ = self.persist();
            info!("Snooze cleared");
        }
        self.get_config()
    }

    /// 置顶指定会话（插入到列表头部，已存在则跳过）
    pub fn pin_session(&self, project_id: String, session_id: String) -> AppConfig {
        if let Ok(mut config) = self.config.write() {
            let pins = config.sessions.pinned_sessions.entry(project_id.clone()).or_default();
            if !pins.iter().any(|p| p.session_id == session_id) {
                pins.insert(0, PinnedSession { session_id: session_id.clone(), pinned_at: now_millis() });
            }
            drop(config);
            let _ = self.persist();
        }
        self.get_config()
    }

    /// 取消置顶指定会话，并清理空的项目条目
    pub fn unpin_session(&self, project_id: String, session_id: String) -> AppConfig {
        if let Ok(mut config) = self.config.write() {
            if let Some(pins) = config.sessions.pinned_sessions.get_mut(&project_id) {
                pins.retain(|p| p.session_id != session_id);
            }
            cleanup_empty_project(&mut config.sessions.pinned_sessions, &project_id);
            drop(config);
            let _ = self.persist();
        }
        self.get_config()
    }

    /// 隐藏指定会话（插入到列表头部，已存在则跳过）
    pub fn hide_session(&self, project_id: String, session_id: String) -> AppConfig {
        if let Ok(mut config) = self.config.write() {
            let hidden = config.sessions.hidden_sessions.entry(project_id.clone()).or_default();
            if !hidden.iter().any(|h| h.session_id == session_id) {
                hidden.insert(0, HiddenSession { session_id: session_id.clone(), hidden_at: now_millis() });
            }
            drop(config);
            let _ = self.persist();
        }
        self.get_config()
    }

    /// 取消隐藏指定会话，并清理空的项目条目
    pub fn unhide_session(&self, project_id: String, session_id: String) -> AppConfig {
        if let Ok(mut config) = self.config.write() {
            if let Some(hidden) = config.sessions.hidden_sessions.get_mut(&project_id) {
                hidden.retain(|h| h.session_id != session_id);
            }
            cleanup_empty_project(&mut config.sessions.hidden_sessions, &project_id);
            drop(config);
            let _ = self.persist();
        }
        self.get_config()
    }

    /// 批量隐藏会话（去重后插入到列表头部）
    pub fn hide_sessions(&self, project_id: String, session_ids: Vec<String>) -> AppConfig {
        if session_ids.is_empty() {
            return self.get_config();
        }
        let ts = now_millis();
        if let Ok(mut config) = self.config.write() {
            let hidden = config.sessions.hidden_sessions.entry(project_id.clone()).or_default();
            // 收集已存在的会话 ID，避免重复
            let existing: std::collections::HashSet<String> =
                hidden.iter().map(|h| h.session_id.clone()).collect();
            let new_entries: Vec<HiddenSession> = session_ids
                .iter()
                .filter(|id| !existing.contains(*id))
                .map(|id| HiddenSession { session_id: id.clone(), hidden_at: ts })
                .collect();
            if !new_entries.is_empty() {
                let mut updated = new_entries;
                updated.extend(hidden.drain(..));
                *hidden = updated;
            }
            drop(config);
            let _ = self.persist();
        }
        self.get_config()
    }

    /// 批量取消隐藏会话，并清理空的项目条目
    pub fn unhide_sessions(&self, project_id: String, session_ids: Vec<String>) -> AppConfig {
        if session_ids.is_empty() {
            return self.get_config();
        }
        let to_remove: std::collections::HashSet<String> = session_ids.into_iter().collect();
        if let Ok(mut config) = self.config.write() {
            if let Some(hidden) = config.sessions.hidden_sessions.get_mut(&project_id) {
                hidden.retain(|h| !to_remove.contains(&h.session_id));
            }
            cleanup_empty_project(&mut config.sessions.hidden_sessions, &project_id);
            drop(config);
            let _ = self.persist();
        }
        self.get_config()
    }

    /// 获取所有通知触发器
    pub fn get_triggers(&self) -> Vec<NotificationTrigger> {
        let config = self.get_config();
        config.notifications.triggers.clone()
    }

    /// 仅获取已启用的通知触发器
    pub fn get_enabled_triggers(&self) -> Vec<NotificationTrigger> {
        self.get_triggers().into_iter().filter(|t| t.enabled).collect()
    }

    /// 添加新的通知触发器。若 ID 已存在则返回错误。
    pub fn add_trigger(
        &self,
        trigger: NotificationTrigger,
    ) -> Result<AppConfig, String> {
        // Validate trigger before persisting
        let validation = crate::infrastructure::trigger_manager::TriggerManager::validate_trigger_only(&trigger);
        if !validation.valid {
            return Err(format!("Invalid trigger: {}", validation.errors.join(", ")));
        }

        let mut config = self
            .config
            .write()
            .map_err(|e| format!("failed to acquire write lock: {e}"))?;

        if config.notifications.triggers.iter().any(|t| t.id == trigger.id) {
            return Err(format!("Trigger with ID '{}' already exists", trigger.id));
        }

        config.notifications.triggers.push(trigger);
        drop(config);
        self.persist()?;
        Ok(self.get_config())
    }

    /// 根据 ID 更新已有的通知触发器。
    /// 使用 camelCase 键名匹配 JSON 格式。
    pub fn update_trigger(
        &self,
        trigger_id: &str,
        updates: serde_json::Value,
    ) -> Result<AppConfig, String> {
        let mut config = self
            .config
            .write()
            .map_err(|e| format!("failed to acquire write lock: {e}"))?;

        let trigger = config.notifications.triggers.iter_mut()
            .find(|t| t.id == trigger_id)
            .ok_or_else(|| format!("Trigger '{}' not found", trigger_id))?;

        // 逐字段应用更新 — 使用 camelCase 键名匹配 JSON 格式
        if let Some(name) = updates.get("name").and_then(|v| v.as_str()) {
            trigger.name = name.to_string();
        }
        if let Some(enabled) = updates.get("enabled").and_then(|v| v.as_bool()) {
            trigger.enabled = enabled;
        }
        if let Some(match_pattern) = updates.get("matchPattern").and_then(|v| v.as_str()) {
            trigger.match_pattern = Some(match_pattern.to_string());
        }
        if let Some(ignore_patterns) = updates.get("ignorePatterns").and_then(|v| v.as_array()) {
            trigger.ignore_patterns = Some(
                ignore_patterns.iter().filter_map(|p| p.as_str().map(String::from)).collect()
            );
        }
        if let Some(token_threshold) = updates.get("tokenThreshold").and_then(|v| v.as_u64()) {
            trigger.token_threshold = Some(token_threshold);
        }
        if let Some(color) = updates.get("color").and_then(|v| v.as_str()) {
            trigger.color = Some(color.to_string());
        }
        if let Some(tool_name) = updates.get("toolName").and_then(|v| v.as_str()) {
            trigger.tool_name = Some(tool_name.to_string());
        }
        if let Some(match_field) = updates.get("matchField").and_then(|v| v.as_str()) {
            trigger.match_field = Some(match_field.to_string());
        }
        if let Some(require_error) = updates.get("requireError").and_then(|v| v.as_bool()) {
            trigger.require_error = Some(require_error);
        }
        if let Some(mode) = updates.get("mode").and_then(|v| v.as_str()) {
            match mode {
                "error_status" => trigger.mode = crate::types::config::TriggerMode::ErrorStatus,
                "content_match" => trigger.mode = crate::types::config::TriggerMode::ContentMatch,
                "token_threshold" => trigger.mode = crate::types::config::TriggerMode::TokenThreshold,
                _ => {}
            }
        }
        if let Some(content_type) = updates.get("contentType").and_then(|v| v.as_str()) {
            match content_type {
                "tool_result" => trigger.content_type = crate::types::config::TriggerContentType::ToolResult,
                "tool_use" => trigger.content_type = crate::types::config::TriggerContentType::ToolUse,
                "thinking" => trigger.content_type = crate::types::config::TriggerContentType::Thinking,
                "text" => trigger.content_type = crate::types::config::TriggerContentType::Text,
                _ => {}
            }
        }
        if let Some(token_type) = updates.get("tokenType").and_then(|v| v.as_str()) {
            match token_type {
                "input" => trigger.token_type = Some(crate::types::config::TriggerTokenType::Input),
                "output" => trigger.token_type = Some(crate::types::config::TriggerTokenType::Output),
                "total" => trigger.token_type = Some(crate::types::config::TriggerTokenType::Total),
                _ => {}
            }
        }
        if let Some(repository_ids) = updates.get("repositoryIds").and_then(|v| v.as_array()) {
            trigger.repository_ids = Some(
                repository_ids.iter().filter_map(|p| p.as_str().map(String::from)).collect()
            );
        }

        // Validate the updated trigger
        let validation = crate::infrastructure::trigger_manager::TriggerManager::validate_trigger_only(trigger);
        if !validation.valid {
            return Err(format!("Invalid trigger update: {}", validation.errors.join(", ")));
        }

        drop(config);
        self.persist()?;
        Ok(self.get_config())
    }

    /// 根据 ID 移除通知触发器。未找到则返回错误。
    pub fn remove_trigger(
        &self,
        trigger_id: &str,
    ) -> Result<AppConfig, String> {
        let mut config = self
            .config
            .write()
            .map_err(|e| format!("failed to acquire write lock: {e}"))?;

        // Guard: builtin triggers cannot be removed, only disabled
        if let Some(trigger) = config.notifications.triggers.iter().find(|t| t.id == trigger_id) {
            if trigger.is_builtin.unwrap_or(false) {
                return Err("Cannot remove built-in triggers. Disable them instead.".to_string());
            }
        }

        let len_before = config.notifications.triggers.len();
        config.notifications.triggers.retain(|t| t.id != trigger_id);

        if config.notifications.triggers.len() == len_before {
            return Err(format!("Trigger '{}' not found", trigger_id));
        }

        drop(config);
        self.persist()?;
        Ok(self.get_config())
    }

    /// 从磁盘加载配置文件并与默认值深度合并。
    /// 文件不存在时直接返回默认配置。
    async fn load_config(&self) -> Result<AppConfig, String> {
        if !self.config_path.exists() {
            info!("No config file found at {:?}, using defaults", self.config_path);
            return Ok(default_app_config());
        }
        let content = fs::read_to_string(&self.config_path)
            .await
            .map_err(|e| format!("failed to read config file: {e}"))?;
        let parsed: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("failed to parse config JSON: {e}"))?;
        merge_with_defaults(&parsed)
    }

    /// 将当前配置持久化到磁盘（格式化 JSON）
    fn persist(&self) -> Result<(), String> {
        let config = self.config.read().map_err(|e| format!("failed to acquire read lock: {e}"))?;
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("failed to create config directory: {e}"))?;
        }
        let content = serde_json::to_string_pretty(&*config).map_err(|e| format!("failed to serialize config: {e}"))?;
        std::fs::write(&self.config_path, content).map_err(|e| format!("failed to write config file: {e}"))?;
        info!("Config saved to {:?}", self.config_path);
        Ok(())
    }
}

// ========== 辅助函数 ==========

/// 将加载的配置与默认值深度合并，确保所有字段都有值
fn merge_with_defaults(loaded: &serde_json::Value) -> Result<AppConfig, String> {
    let defaults = default_config_json();
    let merged = json_merge(&defaults, loaded);
    serde_json::from_value(merged).map_err(|e| format!("failed to deserialize merged config: {e}"))
}

/// 递归深度合并两个 JSON Value。
/// 对于 Object 类型递归合并；非 Object 类型直接用 patch 覆盖 base。
fn json_merge(base: &serde_json::Value, patch: &serde_json::Value) -> serde_json::Value {
    match (base, patch) {
        (serde_json::Value::Object(base_map), serde_json::Value::Object(patch_map)) => {
            let mut merged = base_map.clone();
            for (key, value) in patch_map {
                let entry = merged.remove(key).unwrap_or(serde_json::Value::Null);
                merged.insert(key.clone(), json_merge(&entry, value));
            }
            serde_json::Value::Object(merged)
        }
        (_, patch) => patch.clone(),
    }
}

/// 校验 `config:update` 的 payload，与 Electron 端 `validateConfigUpdatePayload` 对齐。
///
/// 返回 `Ok(())` 表示校验通过，`Err(message)` 表示校验失败。
fn validate_update_payload(section: &str, data: &serde_json::Value) -> Result<(), String> {
    let obj = match data {
        serde_json::Value::Object(map) if !map.is_empty() => map,
        serde_json::Value::Object(_) => return Ok(()), // 空对象，无字段需要校验
        _ => return Err(format!("{section} update must be an object")),
    };

    match section {
        "notifications" => validate_notifications_payload(obj),
        "general" => validate_general_payload(obj),
        "display" => validate_display_payload(obj),
        "httpServer" => validate_http_server_payload(obj),
        "ssh" => validate_ssh_payload(obj),
        "sessions" => Ok(()), // sessions 分区由内部逻辑管理，不对外暴露更新
        _ => Ok(()), // 其他 section 已在 update_config 白名单中拦截
    }
}

/// 校验 notifications 分区的 payload
fn validate_notifications_payload(data: &serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
    let allowed_keys = [
        "enabled",
        "soundEnabled",
        "includeSubagentErrors",
        "ignoredRegex",
        "ignoredRepositories",
        "snoozedUntil",
        "snoozeMinutes",
        "triggers",
    ];

    for key in data.keys() {
        if !allowed_keys.contains(&key.as_str()) {
            return Err(format!("notifications.{key} is not supported via config:update"));
        }
    }

    // enabled, soundEnabled, includeSubagentErrors: must be boolean
    for bool_key in &["enabled", "soundEnabled", "includeSubagentErrors"] {
        if let Some(v) = data.get(*bool_key) {
            if !v.is_boolean() {
                return Err(format!("notifications.{bool_key} must be a boolean"));
            }
        }
    }

    // ignoredRegex: must be string[]
    if let Some(v) = data.get("ignoredRegex") {
        if !v.is_array() || !v.as_array().unwrap().iter().all(|item| item.is_string()) {
            return Err("notifications.ignoredRegex must be a string[]".to_string());
        }
    }

    // ignoredRepositories: must be string[]
    if let Some(v) = data.get("ignoredRepositories") {
        if !v.is_array() || !v.as_array().unwrap().iter().all(|item| item.is_string()) {
            return Err("notifications.ignoredRepositories must be a string[]".to_string());
        }
    }

    // snoozedUntil: must be number >= 0 or null
    if let Some(v) = data.get("snoozedUntil") {
        match v {
            serde_json::Value::Null => {}
            serde_json::Value::Number(n) => {
                if n.as_f64().is_none_or(|f| f.is_nan() || f.is_infinite() || f < 0.0) {
                    return Err("notifications.snoozedUntil must be a non-negative number or null".to_string());
                }
            }
            _ => return Err("notifications.snoozedUntil must be a non-negative number or null".to_string()),
        }
    }

    // snoozeMinutes: must be integer 1-1440
    if let Some(v) = data.get("snoozeMinutes") {
        match v.as_i64() {
            Some(n) => {
                if n < 1 || n > 1440 {
                    return Err("notifications.snoozeMinutes must be between 1 and 1440".to_string());
                }
            }
            None => return Err("notifications.snoozeMinutes must be an integer".to_string()),
        }
    }

    // triggers: must be valid trigger array
    if let Some(v) = data.get("triggers") {
        let arr = match v.as_array() {
            Some(a) => a,
            None => return Err("notifications.triggers must be an array".to_string()),
        };
        for (i, trigger) in arr.iter().enumerate() {
            validate_trigger_payload(trigger).map_err(|e| format!("notifications.triggers[{i}]: {e}"))?;
        }
    }

    Ok(())
}

/// 校验单个 trigger 对象（与 Electron 端 isValidTrigger 对齐）
fn validate_trigger_payload(trigger: &serde_json::Value) -> Result<(), String> {
    let obj = trigger
        .as_object()
        .ok_or("trigger must be an object")?;

    // id: non-empty string
    match obj.get("id").and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => {}
        _ => return Err("trigger.id must be a non-empty string".to_string()),
    }

    // name: non-empty string
    match obj.get("name").and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => {}
        _ => return Err("trigger.name must be a non-empty string".to_string()),
    }

    // enabled: boolean (required)
    match obj.get("enabled").and_then(|v| v.as_bool()) {
        Some(_) => {}
        None => return Err("trigger.enabled must be a boolean".to_string()),
    }

    // contentType: must be one of valid values (required)
    let valid_content_types = ["tool_result", "tool_use", "thinking", "text"];
    match obj.get("contentType").and_then(|v| v.as_str()) {
        Some(s) if valid_content_types.contains(&s) => {}
        _ => return Err("trigger.contentType must be one of: tool_result, tool_use, thinking, text".to_string()),
    }

    // mode: must be one of valid values (required)
    let valid_modes = ["error_status", "content_match", "token_threshold"];
    match obj.get("mode").and_then(|v| v.as_str()) {
        Some(s) if valid_modes.contains(&s) => {}
        _ => return Err("trigger.mode must be one of: error_status, content_match, token_threshold".to_string()),
    }

    Ok(())
}

/// 校验 general 分区的 payload
fn validate_general_payload(data: &serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
    let allowed_keys = [
        "launchAtLogin",
        "showDockIcon",
        "theme",
        "defaultTab",
        "claudeRootPath",
        "autoExpandAIGroups",
        "useNativeTitleBar",
    ];

    for key in data.keys() {
        if !allowed_keys.contains(&key.as_str()) {
            return Err(format!("general.{key} is not a valid setting"));
        }
    }

    // launchAtLogin, showDockIcon, autoExpandAIGroups, useNativeTitleBar: must be boolean
    for bool_key in &["launchAtLogin", "showDockIcon", "autoExpandAIGroups", "useNativeTitleBar"] {
        if let Some(v) = data.get(*bool_key) {
            if !v.is_boolean() {
                return Err(format!("general.{bool_key} must be a boolean"));
            }
        }
    }

    // theme: enum
    if let Some(v) = data.get("theme") {
        let valid = ["dark", "light", "system"];
        match v.as_str() {
            Some(s) if valid.contains(&s) => {}
            _ => return Err("general.theme must be one of: dark, light, system".to_string()),
        }
    }

    // defaultTab: enum
    if let Some(v) = data.get("defaultTab") {
        let valid = ["dashboard", "last-session"];
        match v.as_str() {
            Some(s) if valid.contains(&s) => {}
            _ => return Err("general.defaultTab must be one of: dashboard, last-session".to_string()),
        }
    }

    // claudeRootPath: must be absolute path or null
    if let Some(v) = data.get("claudeRootPath") {
        match v {
            serde_json::Value::Null => {}
            serde_json::Value::String(s) if s.trim().is_empty() => {}
            serde_json::Value::String(s) => {
                if !std::path::Path::new(s.trim()).is_absolute() {
                    return Err("general.claudeRootPath must be an absolute path".to_string());
                }
            }
            _ => return Err("general.claudeRootPath must be an absolute path string or null".to_string()),
        }
    }

    Ok(())
}

/// 校验 display 分区的 payload
fn validate_display_payload(data: &serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
    let allowed_keys = ["showTimestamps", "compactMode", "syntaxHighlighting"];

    for key in data.keys() {
        if !allowed_keys.contains(&key.as_str()) {
            return Err(format!("display.{key} is not a valid setting"));
        }
    }

    // All fields must be boolean
    for bool_key in &allowed_keys {
        if let Some(v) = data.get(*bool_key) {
            if !v.is_boolean() {
                return Err(format!("display.{bool_key} must be a boolean"));
            }
        }
    }

    Ok(())
}

/// 校验 httpServer 分区的 payload
fn validate_http_server_payload(data: &serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
    let allowed_keys = ["enabled", "port"];

    for key in data.keys() {
        if !allowed_keys.contains(&key.as_str()) {
            return Err(format!("httpServer.{key} is not a valid setting"));
        }
    }

    // enabled: must be boolean
    if let Some(v) = data.get("enabled") {
        if !v.is_boolean() {
            return Err("httpServer.enabled must be a boolean".to_string());
        }
    }

    // port: must be integer 1024-65535
    if let Some(v) = data.get("port") {
        match v.as_i64() {
            Some(n) => {
                if n < 1024 || n > 65535 {
                    return Err("httpServer.port must be an integer between 1024 and 65535".to_string());
                }
            }
            None => return Err("httpServer.port must be an integer".to_string()),
        }
    }

    Ok(())
}

/// 校验 ssh 分区的 payload
fn validate_ssh_payload(data: &serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
    let allowed_keys = ["autoReconnect", "lastConnection", "profiles", "lastActiveContextId"];

    for key in data.keys() {
        if !allowed_keys.contains(&key.as_str()) {
            return Err(format!("ssh.{key} is not a valid setting"));
        }
    }

    // autoReconnect: must be boolean
    if let Some(v) = data.get("autoReconnect") {
        if !v.is_boolean() {
            return Err("ssh.autoReconnect must be a boolean".to_string());
        }
    }

    // lastActiveContextId: must be string
    if let Some(v) = data.get("lastActiveContextId") {
        if !v.is_string() {
            return Err("ssh.lastActiveContextId must be a string".to_string());
        }
    }

    // lastConnection: must be object or null
    if let Some(v) = data.get("lastConnection") {
        match v {
            serde_json::Value::Null => {}
            serde_json::Value::Object(_) => {}
            _ => return Err("ssh.lastConnection must be an object or null".to_string()),
        }
    }

    // profiles: must be valid profile array
    if let Some(v) = data.get("profiles") {
        let arr = match v.as_array() {
            Some(a) => a,
            None => return Err("ssh.profiles must be an array".to_string()),
        };
        for (i, profile) in arr.iter().enumerate() {
            validate_ssh_profile_payload(profile).map_err(|e| format!("ssh.profiles[{i}]: {e}"))?;
        }
    }

    Ok(())
}

/// 校验单个 SSH profile 对象
fn validate_ssh_profile_payload(profile: &serde_json::Value) -> Result<(), String> {
    let obj = profile
        .as_object()
        .ok_or("SSH profile must be an object")?;

    // id: non-empty string (required)
    match obj.get("id").and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => {}
        _ => return Err("id must be a non-empty string".to_string()),
    }

    // name: must be string (required)
    match obj.get("name") {
        Some(v) if v.is_string() => {}
        _ => return Err("name must be a string".to_string()),
    }

    // host: must be string (required)
    match obj.get("host") {
        Some(v) if v.is_string() => {}
        _ => return Err("host must be a string".to_string()),
    }

    // port: must be number (required)
    match obj.get("port") {
        Some(v) if v.is_number() => {}
        _ => return Err("port must be a number".to_string()),
    }

    // username: must be string (required)
    match obj.get("username") {
        Some(v) if v.is_string() => {}
        _ => return Err("username must be a string".to_string()),
    }

    // authMethod: must be one of valid values (required)
    let valid_methods = ["password", "privateKey", "agent", "auto"];
    match obj.get("authMethod").and_then(|v| v.as_str()) {
        Some(s) if valid_methods.contains(&s) => {}
        _ => return Err("authMethod must be one of: password, privateKey, agent, auto".to_string()),
    }

    Ok(())
}

/// 更新 JSON 中指定分区的值（深度合并）
fn update_section(current: &serde_json::Value, section: &str, data: &serde_json::Value) -> serde_json::Value {
    let mut updated = current.clone();
    if let Some(current_section) = updated.get_mut(section) {
        *current_section = json_merge(current_section, data);
    } else {
        updated.as_object_mut().map(|map| map.insert(section.to_string(), data.clone()));
    }
    updated
}

/// 清理项目下空的会话列表条目，避免 HashMap 中残留空 Vec
fn cleanup_empty_project<T>(sessions: &mut HashMap<String, Vec<T>>, project_id: &str) {
    if sessions.get(project_id).is_some_and(|v| v.is_empty()) {
        sessions.remove(project_id);
    }
}

/// 规范化 Claude Root Path（与 Electron 的 normalizeConfiguredClaudeRootPath 对齐）。
///
/// 执行以下处理：
/// 1. 解析 `.` 和 `..` 路径段
/// 2. 折叠连续分隔符
/// 3. 去除尾部斜杠（保留根路径 `/`）
fn normalize_claude_root_path(path: &str) -> String {
    let pb = std::path::PathBuf::from(path);
    let mut normalized = std::path::PathBuf::new();

    for comp in pb.components() {
        match comp {
            std::path::Component::CurDir => {} // skip `.`
            std::path::Component::ParentDir => {
                // 回退一级（但不低于根）
                if !normalized.pop() {
                    normalized.push(comp);
                }
            }
            _ => normalized.push(comp),
        }
    }

    let result = normalized.to_string_lossy().to_string();
    // 去除尾部斜杠（保留根路径 "/" 不动）
    let trimmed = result.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_path() -> PathBuf {
        std::env::temp_dir().join(format!("claude-devtools-test-{}.json", uuid::Uuid::new_v4()))
    }

    fn cleanup(p: &PathBuf) { let _ = fs::remove_file(p); }

    #[test]
    fn test_default_config() {
        let c = default_app_config();
        assert!(c.notifications.enabled && c.notifications.sound_enabled);
        assert_eq!(c.notifications.ignored_regex.len(), 1);
        assert!(c.notifications.snoozed_until.is_none());
        assert_eq!(c.notifications.snooze_minutes, 30);
        assert!(!c.general.launch_at_login && c.general.show_dock_icon);
        assert_eq!(c.general.theme, "dark");
        assert!(c.display.show_timestamps && !c.display.compact_mode);
        assert!(c.sessions.pinned_sessions.is_empty());
        assert!(c.ssh.is_some() && c.http_server.is_none());
    }

    #[tokio::test]
    async fn test_save_and_load_round_trip() {
        let p = temp_path(); cleanup(&p);
        let m1 = ConfigManager::with_path(p.clone());
        m1.initialize().await.unwrap();
        m1.pin_session("proj".into(), "sess".into());
        let m2 = ConfigManager::with_path(p.clone());
        m2.initialize().await.unwrap();
        assert_eq!(m2.get_config().sessions.pinned_sessions["proj"][0].session_id, "sess");
        cleanup(&p);
    }

    #[test]
    fn test_add_remove_ignore_regex() {
        let p = temp_path();
        let m = ConfigManager::with_path(p.clone());
        assert!(m.add_ignore_regex("test-pat".into()).unwrap().notifications.ignored_regex.iter().any(|x| x == "test-pat"));
        assert!(m.add_ignore_regex("test-pat".into()).is_err()); // 重复
        assert!(m.add_ignore_regex("(?P<bad".into()).is_err()); // 无效正则
        assert!(m.add_ignore_regex("   ".into()).is_err()); // 空字符串
        assert!(!m.remove_ignore_regex("test-pat".into()).notifications.ignored_regex.iter().any(|x| x == "test-pat"));
        cleanup(&p);
    }

    #[test]
    fn test_pin_unpin_session() {
        let p = temp_path();
        let m = ConfigManager::with_path(p.clone());
        let c = m.pin_session("p".into(), "s1".into());
        assert_eq!(c.sessions.pinned_sessions["p"][0].session_id, "s1");
        let c = m.pin_session("p".into(), "s2".into());
        assert_eq!(c.sessions.pinned_sessions["p"][0].session_id, "s2"); // 插入到头部
        let c = m.pin_session("p".into(), "s1".into()); // 幂等操作
        assert_eq!(c.sessions.pinned_sessions["p"].len(), 2);
        let c = m.unpin_session("p".into(), "s2".into());
        assert_eq!(c.sessions.pinned_sessions["p"].len(), 1); // s1 保留
        cleanup(&p);
    }

    #[test]
    fn test_json_merge_deep() {
        let r = json_merge(&serde_json::json!({"a": 1, "b": {"c": 2, "d": 3}}), &serde_json::json!({"a": 10, "b": {"c": 20}}));
        assert_eq!(r["a"], 10);
        assert_eq!(r["b"]["c"], 20);
        assert_eq!(r["b"]["d"], 3); // 保留未被覆盖的字段
    }

    #[tokio::test]
    async fn test_merge_partial_config() {
        let p = temp_path(); cleanup(&p);
        fs::write(&p, r#"{"general":{"theme":"light"}}"#).unwrap();
        let m = ConfigManager::with_path(p.clone());
        m.initialize().await.unwrap();
        let c = m.get_config();
        assert_eq!(c.general.theme, "light");
        assert!(c.general.show_dock_icon && c.notifications.enabled);
        cleanup(&p);
    }

    #[tokio::test]
    async fn test_initialize_seeds_builtin_triggers() {
        // 首次初始化（无配置文件）应注入 3 个内置触发器
        let p = temp_path(); cleanup(&p);
        let m = ConfigManager::with_path(p.clone());
        m.initialize().await.unwrap();
        let triggers = m.get_triggers();
        assert_eq!(triggers.len(), 3, "should have 3 built-in triggers");
        let ids: Vec<&str> = triggers.iter().map(|t| t.id.as_str()).collect();
        assert!(ids.contains(&"builtin-bash-command"));
        assert!(ids.contains(&"builtin-tool-result-error"));
        assert!(ids.contains(&"builtin-high-token-usage"));
        // 所有内置触发器默认禁用
        for t in &triggers {
            assert!(!t.enabled, "builtin trigger '{}' should be disabled by default", t.id);
            assert_eq!(t.is_builtin, Some(true));
        }
        cleanup(&p);
    }

    #[tokio::test]
    async fn test_initialize_preserves_user_triggers_and_merges_builtins() {
        // 配置文件中有用户触发器 + 修改过的内置触发器
        let p = temp_path(); cleanup(&p);
        let config_json = serde_json::json!({
            "notifications": {
                "triggers": [
                    { "id": "my-custom", "name": "My Trigger", "enabled": true, "contentType": "tool_result", "mode": "error_status" },
                    { "id": "builtin-tool-result-error", "name": "Modified Name", "enabled": true, "contentType": "tool_result", "mode": "error_status" }
                ]
            }
        });
        fs::write(&p, serde_json::to_string(&config_json).unwrap()).unwrap();
        let m = ConfigManager::with_path(p.clone());
        m.initialize().await.unwrap();
        let triggers = m.get_triggers();
        assert_eq!(triggers.len(), 4, "should have 1 user + 1 modified builtin + 2 missing builtins");
        // 用户触发器保留
        let custom = triggers.iter().find(|t| t.id == "my-custom").unwrap();
        assert_eq!(custom.name, "My Trigger");
        assert!(custom.enabled);
        // 修改过的内置触发器保留用户修改
        let modified = triggers.iter().find(|t| t.id == "builtin-tool-result-error").unwrap();
        assert_eq!(modified.name, "Modified Name");
        assert!(modified.enabled);
        // 缺失的内置触发器被补齐
        assert!(triggers.iter().any(|t| t.id == "builtin-bash-command"));
        assert!(triggers.iter().any(|t| t.id == "builtin-high-token-usage"));
        cleanup(&p);
    }

    // ========== validate_update_payload tests ==========

    #[test]
    fn test_validate_rejects_unknown_key_in_notifications() {
        let data = serde_json::json!({"unknownKey": true});
        let err = validate_update_payload("notifications", &data).unwrap_err();
        assert!(err.contains("notifications.unknownKey is not supported"));
    }

    #[test]
    fn test_validate_rejects_unknown_key_in_general() {
        let data = serde_json::json!({"unknownKey": "foo"});
        let err = validate_update_payload("general", &data).unwrap_err();
        assert!(err.contains("general.unknownKey is not a valid setting"));
    }

    #[test]
    fn test_validate_rejects_unknown_key_in_display() {
        let data = serde_json::json!({"unknownKey": 42});
        let err = validate_update_payload("display", &data).unwrap_err();
        assert!(err.contains("display.unknownKey is not a valid setting"));
    }

    #[test]
    fn test_validate_rejects_unknown_key_in_http_server() {
        let data = serde_json::json!({"unknownKey": true});
        let err = validate_update_payload("httpServer", &data).unwrap_err();
        assert!(err.contains("httpServer.unknownKey is not a valid setting"));
    }

    #[test]
    fn test_validate_rejects_unknown_key_in_ssh() {
        let data = serde_json::json!({"unknownKey": true});
        let err = validate_update_payload("ssh", &data).unwrap_err();
        assert!(err.contains("ssh.unknownKey is not a valid setting"));
    }

    #[test]
    fn test_validate_empty_object_passes() {
        assert!(validate_update_payload("notifications", &serde_json::json!({})).is_ok());
        assert!(validate_update_payload("general", &serde_json::json!({})).is_ok());
        assert!(validate_update_payload("display", &serde_json::json!({})).is_ok());
        assert!(validate_update_payload("httpServer", &serde_json::json!({})).is_ok());
        assert!(validate_update_payload("ssh", &serde_json::json!({})).is_ok());
    }

    #[test]
    fn test_validate_non_object_rejects() {
        let err = validate_update_payload("general", &serde_json::json!("string")).unwrap_err();
        assert!(err.contains("must be an object"));
    }

    // --- notifications ---

    #[test]
    fn test_validate_notifications_bool_fields() {
        // valid
        assert!(validate_update_payload("notifications", &serde_json::json!({"enabled": true})).is_ok());
        assert!(validate_update_payload("notifications", &serde_json::json!({"soundEnabled": false})).is_ok());
        assert!(validate_update_payload("notifications", &serde_json::json!({"includeSubagentErrors": true})).is_ok());
        // invalid
        let err = validate_update_payload("notifications", &serde_json::json!({"enabled": "true"})).unwrap_err();
        assert!(err.contains("must be a boolean"));
        let err = validate_update_payload("notifications", &serde_json::json!({"includeSubagentErrors": 1})).unwrap_err();
        assert!(err.contains("must be a boolean"));
    }

    #[test]
    fn test_validate_notifications_snooze_minutes_range() {
        assert!(validate_update_payload("notifications", &serde_json::json!({"snoozeMinutes": 1})).is_ok());
        assert!(validate_update_payload("notifications", &serde_json::json!({"snoozeMinutes": 1440})).is_ok());
        let err = validate_update_payload("notifications", &serde_json::json!({"snoozeMinutes": 0})).unwrap_err();
        assert!(err.contains("between 1 and 1440"));
        let err = validate_update_payload("notifications", &serde_json::json!({"snoozeMinutes": 1441})).unwrap_err();
        assert!(err.contains("between 1 and 1440"));
        let err = validate_update_payload("notifications", &serde_json::json!({"snoozeMinutes": 3.5})).unwrap_err();
        assert!(err.contains("must be an integer"));
    }

    #[test]
    fn test_validate_notifications_ignored_regex_type() {
        assert!(validate_update_payload("notifications", &serde_json::json!({"ignoredRegex": ["pat1", "pat2"]})).is_ok());
        let err = validate_update_payload("notifications", &serde_json::json!({"ignoredRegex": "not-array"})).unwrap_err();
        assert!(err.contains("must be a string[]"));
    }

    #[test]
    fn test_validate_notifications_ignored_repositories_type() {
        assert!(validate_update_payload("notifications", &serde_json::json!({"ignoredRepositories": ["repo1", "repo2"]})).is_ok());
        let err = validate_update_payload("notifications", &serde_json::json!({"ignoredRepositories": "not-array"})).unwrap_err();
        assert!(err.contains("must be a string[]"));
        let err = validate_update_payload("notifications", &serde_json::json!({"ignoredRepositories": [1, 2]})).unwrap_err();
        assert!(err.contains("must be a string[]"));
    }

    #[test]
    fn test_validate_notifications_snoozed_until() {
        assert!(validate_update_payload("notifications", &serde_json::json!({"snoozedUntil": null})).is_ok());
        assert!(validate_update_payload("notifications", &serde_json::json!({"snoozedUntil": 1234567890})).is_ok());
        let err = validate_update_payload("notifications", &serde_json::json!({"snoozedUntil": -1})).unwrap_err();
        assert!(err.contains("non-negative number or null"));
    }

    #[test]
    fn test_validate_notifications_triggers() {
        let valid_trigger = serde_json::json!({"id": "t1", "name": "Test", "enabled": true, "contentType": "tool_result", "mode": "error_status"});
        assert!(validate_update_payload("notifications", &serde_json::json!({"triggers": [valid_trigger]})).is_ok());

        // trigger without id
        let bad_trigger = serde_json::json!({"name": "Test", "enabled": true, "contentType": "tool_result", "mode": "error_status"});
        let err = validate_update_payload("notifications", &serde_json::json!({"triggers": [bad_trigger]})).unwrap_err();
        assert!(err.contains("triggers[0]"));

        // trigger without enabled (required)
        let bad_trigger = serde_json::json!({"id": "t1", "name": "Test", "contentType": "tool_result", "mode": "error_status"});
        let err = validate_update_payload("notifications", &serde_json::json!({"triggers": [bad_trigger]})).unwrap_err();
        assert!(err.contains("enabled must be a boolean"));

        // trigger without contentType (required)
        let bad_trigger = serde_json::json!({"id": "t1", "name": "Test", "enabled": true, "mode": "error_status"});
        let err = validate_update_payload("notifications", &serde_json::json!({"triggers": [bad_trigger]})).unwrap_err();
        assert!(err.contains("contentType must be one of"));

        // invalid contentType
        let bad_content_type = serde_json::json!({"id": "t1", "name": "Test", "enabled": true, "contentType": "invalid", "mode": "error_status"});
        let err = validate_update_payload("notifications", &serde_json::json!({"triggers": [bad_content_type]})).unwrap_err();
        assert!(err.contains("contentType must be one of"));

        // invalid mode
        let bad_mode = serde_json::json!({"id": "t1", "name": "Test", "enabled": true, "contentType": "tool_result", "mode": "invalid"});
        let err = validate_update_payload("notifications", &serde_json::json!({"triggers": [bad_mode]})).unwrap_err();
        assert!(err.contains("mode must be one of"));
    }

    // --- general ---

    #[test]
    fn test_validate_general_theme_enum() {
        assert!(validate_update_payload("general", &serde_json::json!({"theme": "dark"})).is_ok());
        assert!(validate_update_payload("general", &serde_json::json!({"theme": "light"})).is_ok());
        assert!(validate_update_payload("general", &serde_json::json!({"theme": "system"})).is_ok());
        let err = validate_update_payload("general", &serde_json::json!({"theme": "invalid"})).unwrap_err();
        assert!(err.contains("theme must be one of"));
    }

    #[test]
    fn test_validate_general_default_tab_enum() {
        assert!(validate_update_payload("general", &serde_json::json!({"defaultTab": "dashboard"})).is_ok());
        assert!(validate_update_payload("general", &serde_json::json!({"defaultTab": "last-session"})).is_ok());
        let err = validate_update_payload("general", &serde_json::json!({"defaultTab": "invalid"})).unwrap_err();
        assert!(err.contains("defaultTab must be one of"));
    }

    #[test]
    fn test_validate_general_claude_root_path() {
        assert!(validate_update_payload("general", &serde_json::json!({"claudeRootPath": null})).is_ok());
        assert!(validate_update_payload("general", &serde_json::json!({"claudeRootPath": ""})).is_ok());
        assert!(validate_update_payload("general", &serde_json::json!({"claudeRootPath": "/absolute/path"})).is_ok());
        let err = validate_update_payload("general", &serde_json::json!({"claudeRootPath": "relative/path"})).unwrap_err();
        assert!(err.contains("must be an absolute path"));
        // non-string type
        let err = validate_update_payload("general", &serde_json::json!({"claudeRootPath": 42})).unwrap_err();
        assert!(err.contains("must be an absolute path string or null"));
    }

    #[test]
    fn test_validate_general_bool_fields() {
        assert!(validate_update_payload("general", &serde_json::json!({"launchAtLogin": true})).is_ok());
        assert!(validate_update_payload("general", &serde_json::json!({"showDockIcon": false})).is_ok());
        assert!(validate_update_payload("general", &serde_json::json!({"autoExpandAIGroups": true})).is_ok());
        assert!(validate_update_payload("general", &serde_json::json!({"useNativeTitleBar": false})).is_ok());
        let err = validate_update_payload("general", &serde_json::json!({"launchAtLogin": "true"})).unwrap_err();
        assert!(err.contains("must be a boolean"));
    }

    // --- httpServer ---

    #[test]
    fn test_validate_http_server_port_range() {
        assert!(validate_update_payload("httpServer", &serde_json::json!({"port": 1024})).is_ok());
        assert!(validate_update_payload("httpServer", &serde_json::json!({"port": 65535})).is_ok());
        let err = validate_update_payload("httpServer", &serde_json::json!({"port": 0})).unwrap_err();
        assert!(err.contains("between 1024 and 65535"));
        let err = validate_update_payload("httpServer", &serde_json::json!({"port": 1023})).unwrap_err();
        assert!(err.contains("between 1024 and 65535"));
        let err = validate_update_payload("httpServer", &serde_json::json!({"port": 65536})).unwrap_err();
        assert!(err.contains("between 1024 and 65535"));
    }

    // --- ssh ---

    #[test]
    fn test_validate_ssh_auth_method_enum() {
        let valid_profiles = serde_json::json!([{"id": "p1", "name": "Test", "host": "h", "port": 22, "username": "u", "authMethod": "password"}]);
        assert!(validate_update_payload("ssh", &serde_json::json!({"profiles": valid_profiles})).is_ok());

        for valid in &["privateKey", "agent", "auto"] {
            let profiles = serde_json::json!([{"id": "p1", "name": "Test", "host": "h", "port": 22, "username": "u", "authMethod": valid}]);
            assert!(validate_update_payload("ssh", &serde_json::json!({"profiles": profiles})).is_ok());
        }

        let bad_profiles = serde_json::json!([{"id": "p1", "name": "Test", "host": "h", "port": 22, "username": "u", "authMethod": "invalid"}]);
        let err = validate_update_payload("ssh", &serde_json::json!({"profiles": bad_profiles})).unwrap_err();
        assert!(err.contains("authMethod must be one of"));
    }

    #[test]
    fn test_validate_ssh_profile_required_fields() {
        // missing id
        let bad = serde_json::json!([{"name": "Test", "host": "h", "port": 22, "username": "u", "authMethod": "password"}]);
        let err = validate_update_payload("ssh", &serde_json::json!({"profiles": bad})).unwrap_err();
        assert!(err.contains("id must be a non-empty string"));

        // empty id
        let bad = serde_json::json!([{"id": "", "name": "Test"}]);
        let err = validate_update_payload("ssh", &serde_json::json!({"profiles": bad})).unwrap_err();
        assert!(err.contains("id must be a non-empty string"));

        // missing name (required)
        let bad = serde_json::json!([{"id": "p1", "host": "h", "port": 22, "username": "u", "authMethod": "password"}]);
        let err = validate_update_payload("ssh", &serde_json::json!({"profiles": bad})).unwrap_err();
        assert!(err.contains("name must be a string"));

        // missing host (required)
        let bad = serde_json::json!([{"id": "p1", "name": "Test", "port": 22, "username": "u", "authMethod": "password"}]);
        let err = validate_update_payload("ssh", &serde_json::json!({"profiles": bad})).unwrap_err();
        assert!(err.contains("host must be a string"));

        // missing authMethod (required)
        let bad = serde_json::json!([{"id": "p1", "name": "Test", "host": "h", "port": 22, "username": "u"}]);
        let err = validate_update_payload("ssh", &serde_json::json!({"profiles": bad})).unwrap_err();
        assert!(err.contains("authMethod must be one of"));
    }

    #[test]
    fn test_validate_ssh_last_connection_types() {
        assert!(validate_update_payload("ssh", &serde_json::json!({"lastConnection": null})).is_ok());
        assert!(validate_update_payload("ssh", &serde_json::json!({"lastConnection": {}})).is_ok());
        let err = validate_update_payload("ssh", &serde_json::json!({"lastConnection": "string"})).unwrap_err();
        assert!(err.contains("must be an object or null"));
    }

    // --- display ---

    #[test]
    fn test_validate_display_bool_fields() {
        assert!(validate_update_payload("display", &serde_json::json!({"showTimestamps": true})).is_ok());
        let err = validate_update_payload("display", &serde_json::json!({"showTimestamps": "true"})).unwrap_err();
        assert!(err.contains("must be a boolean"));
    }

    // --- sessions ---

    #[test]
    fn test_validate_sessions_always_passes() {
        assert!(validate_update_payload("sessions", &serde_json::json!({"anything": "goes"})).is_ok());
        assert!(validate_update_payload("sessions", &serde_json::json!({})).is_ok());
    }

    // --- claudeRootPath normalization ---

    #[test]
    fn test_normalize_root_path_strips_trailing_slash() {
        assert_eq!(normalize_claude_root_path("/Users/foo/.claude/"), "/Users/foo/.claude");
    }

    #[test]
    fn test_normalize_root_path_resolves_dot_segments() {
        assert_eq!(normalize_claude_root_path("/Users/foo/../bar/.claude"), "/Users/bar/.claude");
    }

    #[test]
    fn test_normalize_root_path_preserves_root() {
        assert_eq!(normalize_claude_root_path("/"), "/");
    }

    #[test]
    fn test_normalize_root_path_no_change_needed() {
        assert_eq!(normalize_claude_root_path("/Users/foo/.claude"), "/Users/foo/.claude");
    }
}
