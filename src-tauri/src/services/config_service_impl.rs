//! ConfigServiceImpl — 含编排逻辑的配置操作具体实现。

use std::sync::Arc;
use async_trait::async_trait;
use tauri::AppHandle;
use tokio::sync::RwLock;

use crate::error::AppError;
use crate::infrastructure::{ConfigManager, DataCache, ContextManager, NotificationManager};
use crate::types::config::{AppConfig, NotificationTrigger, TriggerTestResult};
use crate::services::{ConfigService, SearchServiceFull};

pub struct ConfigServiceImpl {
    config_manager: Arc<ConfigManager>,
    // DataCache 已有 #[derive(Clone)]，无需 Arc 包裹
    cache: DataCache,
    // 所有字段均为 required（单一构造函数，无 builder 模式）
    context_manager: Arc<RwLock<ContextManager>>,
    notification_manager: Arc<RwLock<NotificationManager>>,
    // 直接存储 AppHandle（非 Option），因为组装时必可用
    app_handle: AppHandle,
    search_service: Arc<dyn SearchServiceFull>,
}

impl ConfigServiceImpl {
    /// 单一构造函数，6 个 required 参数。
    pub fn new(
        config_manager: Arc<ConfigManager>,
        cache: DataCache,
        context_manager: Arc<RwLock<ContextManager>>,
        notification_manager: Arc<RwLock<NotificationManager>>,
        app_handle: AppHandle,
        search_service: Arc<dyn SearchServiceFull>,
    ) -> Self {
        Self {
            config_manager,
            cache,
            context_manager,
            notification_manager,
            app_handle,
            search_service,
        }
    }
}

#[async_trait]
impl ConfigService for ConfigServiceImpl {
    async fn update_config(
        &self,
        section: &str,
        data: serde_json::Value,
    ) -> Result<AppConfig, AppError> {
        let has_claude_root_change = section == "general"
            && data.as_object().map_or(false, |obj| obj.contains_key("claudeRootPath"));

        // 【审查修正 #4】透传 AppError，不额外包装。
        // ConfigManager::update_config() 已返回 Result<AppConfig, AppError>，
        // 若再 .map_err(|e| AppError::Config(e)) 会产生双重包装：
        //   AppError::Config(AppError::Io(...)) —— IO 错误被错误标记为 Config
        //   AppError::Config(AppError::Config(...)) —— 丑陋重复
        let result = self.config_manager.update_config(section, data).await?;

        // Rebuild local ServiceContext if claude root path changed (Bug B1 fix)
        // Unified strategy: rebuild failure does NOT block config update,
        // but logs at ERROR level (visible in production monitoring).
        if has_claude_root_change {
            if let Err(e) = crate::infrastructure::context_rebuild::rebuild_local_context(
                &self.context_manager,
                &self.notification_manager,
                &self.config_manager,
                self.cache.clone(),
                &self.app_handle,
                &self.search_service,
            ).await {
                log::error!(
                    "Failed to rebuild local context after claude root path change: {e}"
                );
                // NOT returning error — config update itself succeeded
            }
        }

        Ok(result)
    }

    async fn snooze(&self, minutes: i32) -> Result<AppConfig, AppError> {
        if minutes == -1 {
            // ConfigManager 已返回 Result<AppConfig, AppError>，直接透传（审查修正 #4）
            self.config_manager.snooze_until_tomorrow().await
        } else if minutes <= 0 {
            Err(AppError::InvalidInput("Minutes must be a positive number".into()))
        } else if minutes > 24 * 60 {
            Err(AppError::InvalidInput("Minutes must be 1440 or less (24 hours)".into()))
        } else {
            // ConfigManager 已返回 Result<AppConfig, AppError>，直接透传
            self.config_manager.snooze(minutes as u32).await
        }
    }

    async fn clear_snooze(&self) -> Result<AppConfig, AppError> {
        // ConfigManager 已返回 Result<AppConfig, AppError>，直接透传
        self.config_manager.clear_snooze().await
    }

    async fn test_trigger(
        &self,
        trigger: &NotificationTrigger,
    ) -> Result<TriggerTestResult, AppError> {
        use crate::discovery::project_scanner::ProjectScanner;
        use crate::error::error_trigger_tester;

        let scanner = ProjectScanner::with_paths(
            crate::utils::path_decoder::get_projects_base_path(),
            crate::utils::path_decoder::get_todos_base_path(),
            std::sync::Arc::new(crate::infrastructure::fs_provider::LocalFsProvider::new()),
        );
        Ok(error_trigger_tester::test_trigger(trigger, &scanner, None).await)
    }
}
