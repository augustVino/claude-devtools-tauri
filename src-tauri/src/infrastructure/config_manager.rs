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
        ssh: None,
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
    /// 如果配置文件不存在则使用默认值。
    /// 同时将内置触发器合并到配置中（与 Electron 的 TriggerManager.mergeTriggers 一致）。
    pub async fn initialize(&self) -> Result<(), String> {
        let mut loaded = self.load_config().await?;
        // 合并内置触发器：保留用户修改过的内置触发器，添加缺失的，移除废弃的
        loaded.notifications.triggers = crate::infrastructure::trigger_manager::TriggerManager::merge_triggers(
            loaded.notifications.triggers,
            &crate::infrastructure::trigger_manager::default_triggers(),
        );
        let mut config = self
            .config
            .write()
            .map_err(|e| format!("failed to acquire write lock: {e}"))?;
        *config = loaded;
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
    /// 仅支持 `notifications`、`general`、`display`、`sessions` 四个分区。
    /// 更新后自动与默认值合并，确保新增字段有默认值。
    /// 更新完成后自动持久化到磁盘。
    pub fn update_config(
        &self,
        section: &str,
        data: serde_json::Value,
    ) -> Result<AppConfig, String> {
        let merged: AppConfig = {
            let mut config = self
                .config
                .write()
                .map_err(|e| format!("failed to acquire write lock: {e}"))?;

            let current_json = serde_json::to_value(&*config)
                .map_err(|e| format!("failed to serialize current config: {e}"))?;

            let valid_sections = ["notifications", "general", "display", "sessions"];
            if !valid_sections.contains(&section) {
                return Err(format!("unknown config section: {section}"));
            }

            let updated = update_section(&current_json, section, &data);
            let merged: AppConfig = merge_with_defaults(&updated)?;
            *config = merged.clone();
            merged
        }; // 写锁在此处释放

        self.persist()?; // 持久化可以安全地获取读锁
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
        assert!(c.ssh.is_none() && c.http_server.is_none());
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
}
