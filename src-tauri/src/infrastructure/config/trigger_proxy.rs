//! 触发器 CRUD 委托层 — 复用 TriggerManager 消除重复代码。

use std::sync::Arc;

use crate::error::AppError;
use crate::infrastructure::trigger_manager::TriggerManager;
use crate::types::config::NotificationTrigger;

impl super::ConfigManager {
    /// 获取所有通知触发器
    pub async fn get_triggers(&self) -> Vec<NotificationTrigger> {
        let config = self.get_config().await;
        config.notifications.triggers.clone()
    }

    /// 仅获取已启用的通知触发器
    pub async fn get_enabled_triggers(&self) -> Vec<NotificationTrigger> {
        self.get_triggers().await.into_iter().filter(|t| t.enabled).collect()
    }

    /// 添加新的通知触发器。若 ID 已存在或校验失败则返回错误。
    ///
    /// **保持直接写锁模式**（与 update_trigger/remove_trigger 一致）。
    pub async fn add_trigger(&self, trigger: NotificationTrigger) -> Result<crate::types::AppConfig, AppError> {
        // Phase 1: 校验（无锁，快速失败）
        let validation = TriggerManager::validate_trigger_only(&trigger);
        if !validation.valid {
            return Err(AppError::InvalidInput(format!("Invalid trigger: {}", validation.errors.join(", "))));
        }

        // Phase 2: 写入（直接写锁，原子操作）
        let mut config = self.config.write().await;

        if config.notifications.triggers.iter().any(|t| t.id == trigger.id) {
            return Err(AppError::InvalidInput(format!("Trigger with ID '{}' already exists", trigger.id)));
        }

        config.notifications.triggers.push(trigger);
        drop(config);
        self.persist_inner().await?;
        Ok(self.get_config().await)
    }

    /// 根据 ID 更新已有的通知触发器。
    ///
    /// **保持直接写锁模式**（R1 修正），避免两阶段竞态条件。
    pub async fn update_trigger(
        &self,
        trigger_id: &str,
        updates: serde_json::Value,
    ) -> Result<crate::types::AppConfig, AppError> {
        let mut config = self.config.write().await;

        let current_triggers = config.notifications.triggers.clone();
        let no_op = Arc::new(|| ());
        let mut tm = TriggerManager::new(current_triggers, no_op);

        let updated_triggers = tm.update(trigger_id, updates)
            .map_err(|e| AppError::Config(e))?;

        config.notifications.triggers = updated_triggers;
        drop(config);
        self.persist_inner().await?;
        Ok(self.get_config().await)
    }

    /// 根据 ID 移除通知触发器。内置触发器或未找到则返回错误。
    pub async fn remove_trigger(&self, trigger_id: &str) -> Result<crate::types::AppConfig, AppError> {
        let mut config = self.config.write().await;

        if let Some(trigger) = config.notifications.triggers.iter().find(|t| t.id == trigger_id) {
            if trigger.is_builtin.unwrap_or(false) {
                return Err(AppError::InvalidInput("Cannot remove built-in triggers. Disable them instead.".into()));
            }
        }

        let len_before = config.notifications.triggers.len();
        config.notifications.triggers.retain(|t| t.id != trigger_id);

        if config.notifications.triggers.len() == len_before {
            return Err(AppError::NotFound(format!("Trigger '{trigger_id}' not found")));
        }

        drop(config);
        self.persist_inner().await?;
        Ok(self.get_config().await)
    }
}
