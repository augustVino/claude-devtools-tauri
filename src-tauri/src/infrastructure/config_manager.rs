use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use crate::types::*;
use log::{error, info};
use serde_json;
use tokio::fs;

const CONFIG_FILENAME: &str = "claude-devtools-config.json";
const DEFAULT_IGNORED_REGEX: &[&str] =
    &[r"The user doesn't want to proceed with this tool use\."];

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

fn default_config_json() -> serde_json::Value {
    serde_json::to_value(default_app_config()).expect("default config must serialize")
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub struct ConfigManager {
    config: RwLock<AppConfig>,
    config_path: PathBuf,
}

impl ConfigManager {
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

    pub fn with_path(path: PathBuf) -> Self {
        Self {
            config: RwLock::new(default_app_config()),
            config_path: path,
        }
    }

    pub async fn initialize(&self) -> Result<(), String> {
        let loaded = self.load_config().await?;
        let mut config = self
            .config
            .write()
            .map_err(|e| format!("failed to acquire write lock: {e}"))?;
        *config = loaded;
        Ok(())
    }

    pub fn get_config(&self) -> AppConfig {
        self.config
            .read()
            .map(|c| c.clone())
            .unwrap_or_else(|e| {
                error!("Failed to read config lock, returning defaults: {e}");
                default_app_config()
            })
    }

    pub fn update_config(
        &self,
        section: &str,
        data: serde_json::Value,
    ) -> Result<AppConfig, String> {
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
        Ok(merged)
    }

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
    pub fn remove_ignore_regex(&self, pattern: String) -> AppConfig {
        if let Ok(mut config) = self.config.write() {
            config.notifications.ignored_regex.retain(|p| p != &pattern);
            drop(config);
            let _ = self.persist();
        }
        self.get_config()
    }
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
    pub fn remove_ignore_repository(&self, repo_id: String) -> AppConfig {
        if let Ok(mut config) = self.config.write() {
            config.notifications.ignored_repositories.retain(|id| id != &repo_id);
            drop(config);
            let _ = self.persist();
        }
        self.get_config()
    }
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
    pub fn clear_snooze(&self) -> AppConfig {
        if let Ok(mut config) = self.config.write() {
            config.notifications.snoozed_until = None;
            drop(config);
            let _ = self.persist();
            info!("Snooze cleared");
        }
        self.get_config()
    }
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
    pub fn hide_sessions(&self, project_id: String, session_ids: Vec<String>) -> AppConfig {
        if session_ids.is_empty() {
            return self.get_config();
        }
        let ts = now_millis();
        if let Ok(mut config) = self.config.write() {
            let hidden = config.sessions.hidden_sessions.entry(project_id.clone()).or_default();
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

// Helpers

fn merge_with_defaults(loaded: &serde_json::Value) -> Result<AppConfig, String> {
    let defaults = default_config_json();
    let merged = json_merge(&defaults, loaded);
    serde_json::from_value(merged).map_err(|e| format!("failed to deserialize merged config: {e}"))
}
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
fn update_section(current: &serde_json::Value, section: &str, data: &serde_json::Value) -> serde_json::Value {
    let mut updated = current.clone();
    if let Some(current_section) = updated.get_mut(section) {
        *current_section = json_merge(current_section, data);
    } else {
        updated.as_object_mut().map(|map| map.insert(section.to_string(), data.clone()));
    }
    updated
}
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
        assert!(m.add_ignore_regex("test-pat".into()).is_err()); // dup
        assert!(m.add_ignore_regex("(?P<bad".into()).is_err()); // invalid
        assert!(m.add_ignore_regex("   ".into()).is_err()); // empty
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
        assert_eq!(c.sessions.pinned_sessions["p"][0].session_id, "s2"); // prepended
        let c = m.pin_session("p".into(), "s1".into()); // idempotent
        assert_eq!(c.sessions.pinned_sessions["p"].len(), 2);
        let c = m.unpin_session("p".into(), "s2".into());
        assert_eq!(c.sessions.pinned_sessions["p"].len(), 1); // s1 remains
        cleanup(&p);
    }

    #[test]
    fn test_json_merge_deep() {
        let r = json_merge(&serde_json::json!({"a": 1, "b": {"c": 2, "d": 3}}), &serde_json::json!({"a": 10, "b": {"c": 20}}));
        assert_eq!(r["a"], 10);
        assert_eq!(r["b"]["c"], 20);
        assert_eq!(r["b"]["d"], 3); // preserved
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
}
