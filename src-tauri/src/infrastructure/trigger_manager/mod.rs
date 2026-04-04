//! 触发器管理器 — 管理通知触发器的 CRUD 操作、验证和默认值合并。

pub(crate) mod apply_updates;
pub(crate) mod defaults;
pub(crate) mod validation;

pub use defaults::default_triggers;
pub use crate::types::config::{NotificationTrigger, TriggerValidationResult};

use std::collections::HashSet;
use std::sync::Arc;

use apply_updates::{apply_updates, infer_mode, should_infer_mode};
use validation::validate_trigger_internal;

/// 通知触发器管理器，负责触发器的增删改查与验证。
pub struct TriggerManager {
    triggers: Vec<NotificationTrigger>,
    on_save: Arc<dyn Fn() + Send + Sync>,
}

impl TriggerManager {
    pub fn new(
        triggers: Vec<NotificationTrigger>,
        on_save: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self { triggers, on_save }
    }

    // =========================================================================
    // 读取操作
    // =========================================================================

    /// 获取所有通知触发器。
    pub fn get_all(&self) -> Vec<NotificationTrigger> {
        self.triggers.clone()
    }

    /// 仅获取已启用的通知触发器。
    pub fn get_enabled(&self) -> Vec<NotificationTrigger> {
        self.triggers.iter().filter(|t| t.enabled).cloned().collect()
    }

    /// 按 ID 获取触发器。
    pub fn get_by_id(&self, trigger_id: &str) -> Option<NotificationTrigger> {
        self.triggers.iter().find(|t| t.id == trigger_id).cloned()
    }

    // =========================================================================
    // 写入操作
    // =========================================================================

    /// 添加新的通知触发器。若存在相同 ID 或验证失败则返回错误。
    pub fn add(
        &mut self,
        trigger: NotificationTrigger,
    ) -> Result<Vec<NotificationTrigger>, String> {
        if self.triggers.iter().any(|t| t.id == trigger.id) {
            return Err(format!("Trigger with ID \"{}\" already exists", trigger.id));
        }

        let validation = self.validate(&trigger);
        if !validation.valid {
            return Err(format!("Invalid trigger: {}", validation.errors.join(", ")));
        }

        self.triggers.push(trigger);
        (self.on_save)();
        Ok(self.get_all())
    }

    /// 更新已有的通知触发器。禁止修改内置触发器的 isBuiltin 属性。
    pub fn update(
        &mut self,
        trigger_id: &str,
        updates: serde_json::Value,
    ) -> Result<Vec<NotificationTrigger>, String> {
        let index = self
            .triggers
            .iter()
            .position(|t| t.id == trigger_id)
            .ok_or_else(|| format!("Trigger with ID \"{}\" not found", trigger_id))?;

        let mut updated = self.triggers[index].clone();

        // 从 JSON 值中应用字段更新，过滤掉 isBuiltin 字段。
        apply_updates(&mut updated, &updates);

        // 若未设置 mode 则自动推断（向后兼容）。
        if should_infer_mode(&updates) {
            updated.mode = infer_mode(&updated);
        }

        let validation = self.validate(&updated);
        if !validation.valid {
            return Err(format!(
                "Invalid trigger update: {}",
                validation.errors.join(", ")
            ));
        }

        self.triggers[index] = updated;
        (self.on_save)();
        Ok(self.get_all())
    }

    /// 删除通知触发器。内置触发器不可删除。
    pub fn remove(
        &mut self,
        trigger_id: &str,
    ) -> Result<Vec<NotificationTrigger>, String> {
        let trigger = self
            .triggers
            .iter()
            .find(|t| t.id == trigger_id)
            .ok_or_else(|| format!("Trigger with ID \"{}\" not found", trigger_id))?;

        if trigger.is_builtin == Some(true) {
            return Err("Cannot remove built-in triggers. Disable them instead.".to_string());
        }

        self.triggers.retain(|t| t.id != trigger_id);
        (self.on_save)();
        Ok(self.get_all())
    }

    // =========================================================================
    // 验证（委托给 validation.rs）
    // =========================================================================

    /// 验证触发器配置，不修改状态。
    pub fn validate(&self, trigger: &NotificationTrigger) -> TriggerValidationResult {
        let errors = validate_trigger_internal(trigger);
        TriggerValidationResult { valid: errors.is_empty(), errors }
    }

    /// Validate a trigger without requiring a TriggerManager instance.
    ///
    /// This is used by ConfigManager to validate triggers before persistence,
    /// without needing to construct a full TriggerManager with an on_save callback.
    ///
    /// **[DEPRECATED since v1.0]** Prefer `validate()`. Kept for backward compatibility
    /// with `config/trigger_proxy.rs` (add_trigger validation). Both methods delegate to
    /// `validate_trigger_internal()` — behavior is identical.
    #[deprecated(since = "1.0", note = "Use TriggerManager::validate() instead")]
    #[allow(deprecated)]
    pub fn validate_trigger_only(trigger: &NotificationTrigger) -> TriggerValidationResult {
        let errors = validate_trigger_internal(trigger);
        TriggerValidationResult { valid: errors.is_empty(), errors }
    }

    // =========================================================================
    // 触发器管理
    // =========================================================================

    /// 替换所有触发器（由 ConfigManager 在加载时使用）。
    pub fn set_triggers(&mut self, triggers: Vec<NotificationTrigger>) {
        self.triggers = triggers;
    }

    /// 将已加载的触发器与默认值合并。
    pub fn merge_triggers(
        loaded: Vec<NotificationTrigger>,
        defaults: &[NotificationTrigger],
    ) -> Vec<NotificationTrigger> {
        let builtin_ids: HashSet<&str> = defaults
            .iter()
            .filter(|t| t.is_builtin == Some(true))
            .map(|t| t.id.as_str())
            .collect();

        // 过滤掉已废弃的内置触发器（不在当前默认值中的）。
        let mut merged: Vec<NotificationTrigger> = loaded
            .into_iter()
            .filter(|t| t.is_builtin != Some(true) || builtin_ids.contains(t.id.as_str()))
            .collect();

        // 添加默认值中缺失的内置触发器。
        for default_trigger in defaults {
            if default_trigger.is_builtin == Some(true)
                && !merged.iter().any(|t| t.id == default_trigger.id)
            {
                merged.push(default_trigger.clone());
            }
        }

        merged
    }
}

#[cfg(test)]
mod tests;
