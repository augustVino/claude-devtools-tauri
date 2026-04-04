//! 触发器 CRUD 委托层 — 复用 TriggerManager 消除重复代码。

use std::sync::Arc;

use crate::infrastructure::trigger_manager::TriggerManager;
use crate::types::config::NotificationTrigger;

impl super::ConfigManager {
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
    pub fn add_trigger(&self, trigger: NotificationTrigger) -> Result<crate::types::AppConfig, String> {
        let validation = TriggerManager::validate_trigger_only(&trigger);
        if !validation.valid { return Err(format!("Invalid trigger: {}", validation.errors.join(", "))); }

        let mut config = self.config.write()
            .map_err(|e| format!("failed to acquire write lock: {e}"))?;

        if config.notifications.triggers.iter().any(|t| t.id == trigger.id) {
            return Err(format!("Trigger with ID '{}' already exists", trigger.id));
        }

        config.notifications.triggers.push(trigger);
        drop(config); self.persist()?; Ok(self.get_config())
    }

    /// 根据 ID 更新已有的通知触发器。
    ///
    /// **委托给 TriggerManager::update()** 以复用 apply_updates + should_infer_mode/infer_mode + validate。
    /// 消除原先 84 行逐字段手动更新的重复代码。
    ///
    /// 行为变更说明：委托后引入 infer_mode 行为（当 updates 中不含 mode 字段时自动推断），
    /// 这与 Electron 端行为一致，视为功能修复。
    pub fn update_trigger(
        &self,
        trigger_id: &str,
        updates: serde_json::Value,
    ) -> Result<crate::types::AppConfig, String> {
        let mut config = self.config.write()
            .map_err(|e| format!("failed to acquire write lock: {e}"))?;

        let current_triggers = config.notifications.triggers.clone();
        let no_op = Arc::new(|| ());
        let mut tm = TriggerManager::new(current_triggers, no_op);

        // 委托：内部执行 apply_updates + should_infer_mode/infer_mode + validate
        let updated_triggers = tm.update(trigger_id, updates).map_err(|e| e)?;

        config.notifications.triggers = updated_triggers;
        drop(config); self.persist()?; Ok(self.get_config())
    }

    /// 根据 ID 移除通知触发器。未找到则返回错误。
    ///
    /// **保持手动实现**，不委托给 TriggerManager::remove()。
    /// 原因：当前实现包含内置守卫逻辑（检查 is_builtin + 返回业务错误消息），
    /// 与 TriggerManager::remove() 的语义不同（后者仅检查 builtin 标志返回不同错误文本）。
    /// 手动实现仅 ~26 行，委托收益不足以抵消语义差异风险。
    pub fn remove_trigger(&self, trigger_id: &str) -> Result<crate::types::AppConfig, String> {
        let mut config = self.config.write()
            .map_err(|e| format!("failed to acquire write lock: {e}"))?;

        // Guard: builtin triggers cannot be removed, only disabled
        if let Some(trigger) = config.notifications.triggers.iter().find(|t| t.id == trigger_id) {
            if trigger.is_builtin.unwrap_or(false) {
                return Err("Cannot remove built-in triggers. Disable them instead.".into());
            }
        }

        let len_before = config.notifications.triggers.len();
        config.notifications.triggers.retain(|t| t.id != trigger_id);

        if config.notifications.triggers.len() == len_before {
            return Err(format!("Trigger '{trigger_id}' not found"));
        }

        drop(config); self.persist()?; Ok(self.get_config())
    }
}
