//! 配置管理器 — 管理应用配置的加载、合并、分区更新和持久化。
//!
//! 配置文件路径: `~/.claude/claude-devtools-config.json`

mod defaults;
mod path_utils;

// 子模块 impl 块（按职责分离）
mod ignore_list;
mod session_ops;
mod snooze;
mod trigger_proxy;

use defaults::{CONFIG_FILENAME, DEFAULT_IGNORED_REGEX, default_app_config, default_config_json, now_millis};

use std::path::PathBuf;
use tokio::sync::RwLock;

use crate::types::*;
use crate::error::AppError;
use log::{error, info};
use serde_json;
use tokio::fs;

use super::config_validator;

/// 应用配置管理器
///
/// 负责从磁盘加载配置、深度合并默认值、分区更新和持久化。
/// 内部使用 `RwLock` 保证线程安全的读写访问。
/// （Phase 3 将迁移为 tokio::sync::RwLock）
pub struct ConfigManager {
    /// 当前配置（受读写锁保护）
    config: RwLock<AppConfig>,
    /// 配置文件路径
    config_path: PathBuf,
}

impl ConfigManager {
    /// 使用默认路径创建配置管理器
    pub fn new() -> Self {
        let config_path = dirs::home_dir()
            .expect("home directory must exist")
            .join(".claude")
            .join(CONFIG_FILENAME);
        Self { config: RwLock::new(default_app_config()), config_path }
    }

    /// 使用自定义路径创建配置管理器（主要用于测试）
    pub fn with_path(path: PathBuf) -> Self {
        Self { config: RwLock::new(default_app_config()), config_path: path }
    }

    /// 初始化：从磁盘加载配置并与默认值深度合并。
    pub async fn initialize(&self) -> Result<(), String> {
        let file_existed = self.config_path.exists();
        let mut loaded = self.load_config().await?;
        loaded.notifications.triggers = crate::infrastructure::trigger_manager::TriggerManager::merge_triggers(
            loaded.notifications.triggers,
            &crate::infrastructure::trigger_manager::default_triggers(),
        );
        {
            let mut config = self.config.write().await;
            *config = loaded;
        }
        if !file_existed {
            self.persist().await?;
            info!("Created default config file at {:?}", self.config_path);
        }
        Ok(())
    }

    /// 返回配置文件的路径
    pub fn get_config_path(&self) -> std::path::PathBuf { self.config_path.clone() }

    /// 获取当前配置的完整副本。（现为 async）
    pub async fn get_config(&self) -> AppConfig {
        self.config.read().await.clone()
    }

    /// 分区更新配置。支持六个分区，含字段级校验和 claudeRootPath 规范化。（现为 async）
    pub async fn update_config(&self, section: &str, mut data: serde_json::Value) -> Result<AppConfig, String> {
        let merged: AppConfig = {
            let mut config = self.config.write().await;
            let current_json = serde_json::to_value(&*config)
                .map_err(|e| format!("failed to serialize current config: {e}"))?;
            let valid_sections = ["notifications", "general", "display", "sessions", "ssh", "httpServer"];
            if !valid_sections.contains(&section) {
                return Err(format!("unknown config section: {section}"));
            }
            config_validator::validate_update_payload(section, &data)?;
            if section == "general" {
                if let Some(obj) = data.as_object_mut() {
                    if let Some(v) = obj.get_mut("claudeRootPath") {
                        if let Some(s) = v.as_str() {
                            let trimmed = s.trim();
                            if !trimmed.is_empty() { *v = serde_json::Value::String(path_utils::normalize_claude_root_path(trimmed)); }
                        }
                    }
                }
            }
            let updated = config_validator::update_section(&current_json, section, &data);
            let merged: AppConfig = merge_with_defaults(&updated)?;
            *config = merged.clone();
            merged
        };
        self.persist().await?;
        if section == "general" { crate::utils::set_claude_root_override(merged.general.claude_root_path.clone()); }
        Ok(merged)
    }

    // ========== 持久化（内联，逻辑简单）==========

    async fn load_config(&self) -> Result<AppConfig, String> {
        if !self.config_path.exists() {
            info!("No config file found at {:?}, using defaults", self.config_path);
            return Ok(default_app_config());
        }
        let content = fs::read_to_string(&self.config_path).await
            .map_err(|e| format!("failed to read config file: {e}"))?;
        let parsed: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("failed to parse config JSON: {e}"))?;
        merge_with_defaults(&parsed)
    }

    async fn persist_inner(&self) -> Result<(), AppError> {
        let config = self.config.read().await;
        if let Some(parent) = self.config_path.parent() { tokio::fs::create_dir_all(parent).await.map_err(|e| AppError::Io(e))?; }
        let content = serde_json::to_string_pretty(&*config)
            .map_err(|e| AppError::Config(format!("failed to serialize config: {e}")))?;
        tokio::fs::write(&self.config_path, content).await
            .map_err(|e| AppError::Io(e))?;
        info!("Config saved to {:?}", self.config_path);
        Ok(())
    }

    async fn persist(&self) -> Result<(), String> { self.persist_inner().await.map_err(|e| e.to_string()) }
}

#[cfg(test)]
mod tests;

// ========== 委托到 config_validator 的薄封装 ==========

fn merge_with_defaults(loaded: &serde_json::Value) -> Result<AppConfig, String> {
    let defaults = defaults::default_config_json();
    let merged = config_validator::json_merge(&defaults, loaded);
    serde_json::from_value(merged).map_err(|e| format!("failed to deserialize merged config: {e}"))
}
