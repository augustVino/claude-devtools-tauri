//! 触发器管理器 — 管理通知触发器。
//!
//! 处理通知触发器的 CRUD 操作，包括:
//! - 添加、更新和删除触发器
//! - 验证触发器配置（含 ReDoS 防护）
//! - 管理内置触发器与自定义触发器
//! - 将已加载的触发器与默认值合并

use std::collections::HashSet;
use std::sync::Arc;

use crate::types::config::{
    NotificationTrigger, TriggerContentType, TriggerMode, TriggerTokenType,
    TriggerValidationResult,
};
use crate::utils::regex_validation::validate_regex_pattern;

// =============================================================================
// 默认触发器
// =============================================================================

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

// =============================================================================
// TriggerManager
// =============================================================================

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

    /// 添加新的通知触发器。
    /// 若存在相同 ID 的触发器或验证失败则返回错误。
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

    /// 更新已有的通知触发器。
    /// 禁止修改内置触发器的 isBuiltin 属性。
    /// 若触发器不存在或验证失败则返回错误。
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

    /// 删除通知触发器。
    /// 内置触发器不可删除。
    /// 若触发器不存在或是内置触发器则返回错误。
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
    // 验证
    // =========================================================================

    /// Validate a trigger without requiring a TriggerManager instance.
    ///
    /// This is used by ConfigManager to validate triggers before persistence,
    /// without needing to construct a full TriggerManager with an on_save callback.
    pub fn validate_trigger_only(trigger: &NotificationTrigger) -> TriggerValidationResult {
        let mut errors = Vec::new();

        // 必填字段检查。
        if trigger.id.trim().is_empty() {
            errors.push("Trigger ID is required".to_string());
        }

        if trigger.name.trim().is_empty() {
            errors.push("Trigger name is required".to_string());
        }

        // 模式特定的验证。
        match &trigger.mode {
            TriggerMode::ContentMatch => {
                // match_field 为必填，除非是 tool_use 且无指定工具名（匹配任意工具）。
                if trigger.match_field.is_none()
                    && !(trigger.content_type == TriggerContentType::ToolUse
                        && trigger.tool_name.is_none())
                {
                    errors.push("Match field is required for content_match mode".to_string());
                }
                // 验证正则模式（含 ReDoS 防护）。
                if let Some(pattern) = &trigger.match_pattern {
                    let validation = validate_regex_pattern(pattern);
                    if !validation.valid {
                        errors.push(
                            validation
                                .error
                                .map(|e| e.reason)
                                .unwrap_or_else(|| "Invalid regex pattern".to_string()),
                        );
                    }
                }
            }
            TriggerMode::TokenThreshold => {
                if trigger.token_threshold.is_none() {
                    errors.push("Token threshold must be a non-negative number".to_string());
                }
                if trigger.token_type.is_none() {
                    errors.push("Token type is required for token_threshold mode".to_string());
                }
            }
            TriggerMode::ErrorStatus => {
                // error_status 模式无额外要求。
            }
        }

        // 验证忽略模式（含 ReDoS 防护）。
        if let Some(patterns) = &trigger.ignore_patterns {
            for pattern in patterns {
                let validation = validate_regex_pattern(pattern);
                if !validation.valid {
                    errors.push(format!(
                        "Invalid ignore pattern \"{}\": {}",
                        pattern,
                        validation
                            .error
                            .map(|e| e.reason)
                            .unwrap_or_else(|| "Unknown error".to_string())
                    ));
                }
            }
        }

        TriggerValidationResult {
            valid: errors.is_empty(),
            errors,
        }
    }

    /// 验证触发器配置，不修改状态。
    pub fn validate(&self, trigger: &NotificationTrigger) -> TriggerValidationResult {
        let mut errors = Vec::new();

        // 必填字段检查。
        if trigger.id.trim().is_empty() {
            errors.push("Trigger ID is required".to_string());
        }

        if trigger.name.trim().is_empty() {
            errors.push("Trigger name is required".to_string());
        }

        // 模式特定的验证。
        match &trigger.mode {
            TriggerMode::ContentMatch => {
                // match_field 为必填，除非是 tool_use 且无指定工具名（匹配任意工具）。
                if trigger.match_field.is_none()
                    && !(trigger.content_type == TriggerContentType::ToolUse
                        && trigger.tool_name.is_none())
                {
                    errors.push("Match field is required for content_match mode".to_string());
                }
                // 验证正则模式（含 ReDoS 防护）。
                if let Some(pattern) = &trigger.match_pattern {
                    let validation = validate_regex_pattern(pattern);
                    if !validation.valid {
                        errors.push(
                            validation
                                .error
                                .map(|e| e.reason)
                                .unwrap_or_else(|| "Invalid regex pattern".to_string()),
                        );
                    }
                }
            }
            TriggerMode::TokenThreshold => {
                if trigger.token_threshold.is_none() {
                    errors.push("Token threshold must be a non-negative number".to_string());
                }
                if trigger.token_type.is_none() {
                    errors.push("Token type is required for token_threshold mode".to_string());
                }
            }
            TriggerMode::ErrorStatus => {
                // error_status 模式无额外要求。
            }
        }

        // 验证忽略模式（含 ReDoS 防护）。
        if let Some(patterns) = &trigger.ignore_patterns {
            for pattern in patterns {
                let validation = validate_regex_pattern(pattern);
                if !validation.valid {
                    errors.push(format!(
                        "Invalid ignore pattern \"{}\": {}",
                        pattern,
                        validation
                            .error
                            .map(|e| e.reason)
                            .unwrap_or_else(|| "Unknown error".to_string())
                    ));
                }
            }
        }

        TriggerValidationResult {
            valid: errors.is_empty(),
            errors,
        }
    }

    // =========================================================================
    // 触发器管理
    // =========================================================================

    /// 替换所有触发器（由 ConfigManager 在加载时使用）。
    pub fn set_triggers(&mut self, triggers: Vec<NotificationTrigger>) {
        self.triggers = triggers;
    }

    /// 将已加载的触发器与默认值合并。
    /// - 保留所有现有触发器（包括用户修改过的内置触发器）。
    /// - 添加默认值中缺失的内置触发器。
    /// - 移除已废弃且不在当前默认值中的内置触发器。
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

// =============================================================================
// 内部辅助函数
// =============================================================================

/// 将 JSON 值中的字段更新应用到触发器，过滤掉 `isBuiltin` 字段。
fn apply_updates(trigger: &mut NotificationTrigger, updates: &serde_json::Value) {
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
            ignore_patterns
                .iter()
                .filter_map(|p| p.as_str().map(String::from))
                .collect(),
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
    if let Some(content_type) = updates.get("contentType").and_then(|v| v.as_str()) {
        if let Ok(ct) = serde_json::from_value(serde_json::json!(content_type)) {
            trigger.content_type = ct;
        }
    }
    if let Some(mode) = updates.get("mode").and_then(|v| v.as_str()) {
        if let Ok(m) = serde_json::from_value(serde_json::json!(mode)) {
            trigger.mode = m;
        }
    }
    // 注意: `isBuiltin` 被有意忽略 — 内置状态不可更改。
}

/// 判断是否需要进行模式推断（更新中未包含 mode 字段）。
fn should_infer_mode(updates: &serde_json::Value) -> bool {
    !updates.get("mode").map_or(false, |v| v.is_string())
}

/// 根据触发器属性推断模式，用于向后兼容。
fn infer_mode(trigger: &NotificationTrigger) -> TriggerMode {
    if trigger.require_error == Some(true) {
        return TriggerMode::ErrorStatus;
    }
    if trigger.match_pattern.is_some() || trigger.match_field.is_some() {
        return TriggerMode::ContentMatch;
    }
    if trigger.token_threshold.is_some() {
        return TriggerMode::TokenThreshold;
    }
    TriggerMode::ErrorStatus // 默认回退
}

// =============================================================================
// 测试
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn no_op() {}
    fn make_manager(triggers: Vec<NotificationTrigger>) -> TriggerManager {
        TriggerManager::new(triggers, Arc::new(no_op))
    }

    fn custom_trigger(id: &str, name: &str) -> NotificationTrigger {
        NotificationTrigger {
            id: id.to_string(),
            name: name.to_string(),
            enabled: true,
            content_type: TriggerContentType::ToolResult,
            mode: TriggerMode::ErrorStatus,
            require_error: Some(true),
            tool_name: None,
            match_field: None,
            match_pattern: None,
            token_threshold: None,
            token_type: None,
            ignore_patterns: None,
            is_builtin: None,
            color: Some("blue".to_string()),
            repository_ids: None,
        }
    }

    fn content_match_trigger(id: &str, name: &str, pattern: &str) -> NotificationTrigger {
        NotificationTrigger {
            id: id.to_string(),
            name: name.to_string(),
            enabled: true,
            content_type: TriggerContentType::ToolUse,
            mode: TriggerMode::ContentMatch,
            match_pattern: Some(pattern.to_string()),
            match_field: Some("input".to_string()),
            tool_name: None,
            require_error: None,
            token_threshold: None,
            token_type: None,
            ignore_patterns: None,
            is_builtin: None,
            color: None,
            repository_ids: None,
        }
    }

    fn token_threshold_trigger(id: &str, name: &str, threshold: u64) -> NotificationTrigger {
        NotificationTrigger {
            id: id.to_string(),
            name: name.to_string(),
            enabled: true,
            content_type: TriggerContentType::ToolResult,
            mode: TriggerMode::TokenThreshold,
            token_threshold: Some(threshold),
            token_type: Some(TriggerTokenType::Total),
            tool_name: None,
            match_field: None,
            match_pattern: None,
            require_error: None,
            ignore_patterns: None,
            is_builtin: None,
            color: None,
            repository_ids: None,
        }
    }

    // =========================================================================
    // default_triggers
    // =========================================================================

    #[test]
    fn test_default_triggers_count() {
        let triggers = default_triggers();
        assert_eq!(triggers.len(), 3);
    }

    #[test]
    fn test_default_triggers_ids() {
        let triggers = default_triggers();
        assert_eq!(triggers[0].id, "builtin-bash-command");
        assert_eq!(triggers[1].id, "builtin-tool-result-error");
        assert_eq!(triggers[2].id, "builtin-high-token-usage");
    }

    #[test]
    fn test_default_triggers_all_builtin() {
        let triggers = default_triggers();
        for t in &triggers {
            assert_eq!(t.is_builtin, Some(true));
        }
    }

    #[test]
    fn test_default_triggers_all_disabled() {
        let triggers = default_triggers();
        for t in &triggers {
            assert!(!t.enabled);
        }
    }

    // =========================================================================
    // get_all, get_enabled, get_by_id
    // =========================================================================

    #[test]
    fn test_get_all_returns_all() {
        let triggers = default_triggers();
        let manager = make_manager(triggers);
        assert_eq!(manager.get_all().len(), 3);
    }

    #[test]
    fn test_get_enabled_filters_correctly() {
        let mut triggers = default_triggers();
        triggers[0].enabled = true;
        let manager = make_manager(triggers);
        let enabled = manager.get_enabled();
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].id, "builtin-bash-command");
    }

    #[test]
    fn test_get_by_id_found() {
        let manager = make_manager(default_triggers());
        let trigger = manager.get_by_id("builtin-tool-result-error");
        assert!(trigger.is_some());
        assert_eq!(trigger.unwrap().name, "Tool Result Error");
    }

    #[test]
    fn test_get_by_id_not_found() {
        let manager = make_manager(default_triggers());
        assert!(manager.get_by_id("nonexistent").is_none());
    }

    // =========================================================================
    // add
    // =========================================================================

    #[test]
    fn test_add_valid_trigger() {
        let mut manager = make_manager(default_triggers());
        let trigger = custom_trigger("custom-1", "My Custom Trigger");
        let result = manager.add(trigger).unwrap();
        assert_eq!(result.len(), 4);
        assert_eq!(result[3].id, "custom-1");
    }

    #[test]
    fn test_add_duplicate_id_fails() {
        let mut manager = make_manager(default_triggers());
        let trigger = custom_trigger("builtin-bash-command", "Duplicate");
        let result = manager.add(trigger);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already exists"));
    }

    #[test]
    fn test_add_invalid_trigger_fails() {
        let mut manager = make_manager(default_triggers());
        let trigger = NotificationTrigger {
            id: "bad-trigger".to_string(),
            name: "".to_string(), // 空名称
            enabled: true,
            content_type: TriggerContentType::ToolResult,
            mode: TriggerMode::ErrorStatus,
            require_error: None,
            tool_name: None,
            match_field: None,
            match_pattern: None,
            token_threshold: None,
            token_type: None,
            ignore_patterns: None,
            is_builtin: None,
            color: None,
            repository_ids: None,
        };
        let result = manager.add(trigger);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Trigger name is required"));
    }

    #[test]
    fn test_add_content_match_without_match_field_fails() {
        let mut manager = make_manager(default_triggers());
        let trigger = NotificationTrigger {
            id: "cm-no-field".to_string(),
            name: "Bad Content Match".to_string(),
            enabled: true,
            content_type: TriggerContentType::ToolResult,
            mode: TriggerMode::ContentMatch,
            match_pattern: Some("test".to_string()),
            match_field: None, // tool_result 的 content_match 模式缺少此字段
            tool_name: None,
            require_error: None,
            token_threshold: None,
            token_type: None,
            ignore_patterns: None,
            is_builtin: None,
            color: None,
            repository_ids: None,
        };
        let result = manager.add(trigger);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Match field is required"));
    }

    // =========================================================================
    // update
    // =========================================================================

    #[test]
    fn test_update_trigger_name() {
        let mut manager = make_manager(default_triggers());
        let updates = serde_json::json!({"name": "Updated Name"});
        let result = manager.update("builtin-bash-command", updates).unwrap();
        assert_eq!(result[0].name, "Updated Name");
    }

    #[test]
    fn test_update_builtin_cannot_change_is_builtin() {
        let mut manager = make_manager(default_triggers());
        let updates = serde_json::json!({"isBuiltin": false});
        let result = manager.update("builtin-bash-command", updates).unwrap();
        // isBuiltin 应保持 true — 该字段被 apply_updates 忽略。
        assert_eq!(result[0].is_builtin, Some(true));
    }

    #[test]
    fn test_update_nonexistent_fails() {
        let mut manager = make_manager(default_triggers());
        let updates = serde_json::json!({"name": "Nope"});
        let result = manager.update("nonexistent", updates);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_update_to_invalid_state_fails() {
        let mut manager = make_manager(default_triggers());
        let updates = serde_json::json!({"name": ""});
        let result = manager.update("builtin-bash-command", updates);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Trigger name is required"));
    }

    // =========================================================================
    // remove
    // =========================================================================

    #[test]
    fn test_remove_custom_trigger() {
        let mut triggers = default_triggers();
        triggers.push(custom_trigger("custom-1", "Custom"));
        let mut manager = make_manager(triggers);
        let result = manager.remove("custom-1").unwrap();
        assert_eq!(result.len(), 3);
        assert!(result.iter().all(|t| t.id != "custom-1"));
    }

    #[test]
    fn test_remove_builtin_trigger_fails() {
        let mut manager = make_manager(default_triggers());
        let result = manager.remove("builtin-bash-command");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Cannot remove built-in triggers"));
    }

    #[test]
    fn test_remove_nonexistent_fails() {
        let mut manager = make_manager(default_triggers());
        let result = manager.remove("nonexistent");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    // =========================================================================
    // validate
    // =========================================================================

    #[test]
    fn test_validate_valid_error_status_trigger() {
        let manager = make_manager(default_triggers());
        let trigger = custom_trigger("test-1", "Valid Error Status");
        let result = manager.validate(&trigger);
        assert!(result.valid);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_validate_valid_content_match_trigger() {
        let manager = make_manager(default_triggers());
        let trigger = content_match_trigger("test-2", "Valid Content Match", r"\.env$");
        let result = manager.validate(&trigger);
        assert!(result.valid);
    }

    #[test]
    fn test_validate_valid_token_threshold_trigger() {
        let manager = make_manager(default_triggers());
        let trigger = token_threshold_trigger("test-3", "Valid Token Threshold", 5000);
        let result = manager.validate(&trigger);
        assert!(result.valid);
    }

    #[test]
    fn test_validate_empty_name() {
        let manager = make_manager(default_triggers());
        let trigger = custom_trigger("test-empty", "");
        let result = manager.validate(&trigger);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.contains("name")));
    }

    #[test]
    fn test_validate_content_match_missing_match_field() {
        let manager = make_manager(default_triggers());
        let trigger = NotificationTrigger {
            id: "test-cm".to_string(),
            name: "Bad CM".to_string(),
            enabled: true,
            content_type: TriggerContentType::ToolResult,
            mode: TriggerMode::ContentMatch,
            match_pattern: Some("test".to_string()),
            match_field: None,
            tool_name: None,
            require_error: None,
            token_threshold: None,
            token_type: None,
            ignore_patterns: None,
            is_builtin: None,
            color: None,
            repository_ids: None,
        };
        let result = manager.validate(&trigger);
        assert!(!result.valid);
    }

    #[test]
    fn test_validate_content_match_tool_use_without_tool_name_ok() {
        let manager = make_manager(default_triggers());
        // ToolUse 无 toolName 应合法（匹配任意工具）。
        let trigger = NotificationTrigger {
            id: "test-cm-tu".to_string(),
            name: "CM ToolUse Any".to_string(),
            enabled: true,
            content_type: TriggerContentType::ToolUse,
            mode: TriggerMode::ContentMatch,
            match_pattern: Some("test".to_string()),
            match_field: None,
            tool_name: None,
            require_error: None,
            token_threshold: None,
            token_type: None,
            ignore_patterns: None,
            is_builtin: None,
            color: None,
            repository_ids: None,
        };
        let result = manager.validate(&trigger);
        assert!(result.valid);
    }

    #[test]
    fn test_validate_token_threshold_zero_passes() {
        let manager = make_manager(default_triggers());
        let trigger = token_threshold_trigger("test-zero", "Zero Threshold", 0);
        let result = manager.validate(&trigger);
        assert!(result.valid);
    }

    #[test]
    fn test_validate_token_threshold_missing_type_fails() {
        let manager = make_manager(default_triggers());
        let trigger = NotificationTrigger {
            id: "test-tt-notype".to_string(),
            name: "Missing Type".to_string(),
            enabled: true,
            content_type: TriggerContentType::ToolResult,
            mode: TriggerMode::TokenThreshold,
            token_threshold: Some(1000),
            token_type: None, // 缺失
            tool_name: None,
            match_field: None,
            match_pattern: None,
            require_error: None,
            ignore_patterns: None,
            is_builtin: None,
            color: None,
            repository_ids: None,
        };
        let result = manager.validate(&trigger);
        assert!(!result.valid);
        assert!(result
            .errors
            .iter()
            .any(|e| e.contains("Token type is required")));
    }

    #[test]
    fn test_validate_invalid_regex_pattern() {
        let manager = make_manager(default_triggers());
        let trigger = content_match_trigger("test-bad-regex", "Bad Regex", r"(?P<unclosed");
        let result = manager.validate(&trigger);
        assert!(!result.valid);
    }

    #[test]
    fn test_validate_invalid_ignore_pattern() {
        let manager = make_manager(default_triggers());
        let mut trigger = custom_trigger("test-bad-ignore", "Bad Ignore");
        trigger.ignore_patterns = Some(vec![r"(?P<bad".to_string()]);
        let result = manager.validate(&trigger);
        assert!(!result.valid);
        assert!(result
            .errors
            .iter()
            .any(|e| e.contains("Invalid ignore pattern")));
    }

    // =========================================================================
    // merge_triggers
    // =========================================================================

    #[test]
    fn test_merge_triggers_adds_missing_builtins() {
        let loaded = vec![custom_trigger("custom-1", "My Custom")];
        let defaults = default_triggers();
        let merged = TriggerManager::merge_triggers(loaded, &defaults);
        assert_eq!(merged.len(), 4); // 1 custom + 3 builtins
    }

    #[test]
    fn test_merge_triggers_preserves_existing() {
        let mut loaded = default_triggers();
        loaded[0].enabled = true; // 用户启用了第一个内置触发器
        let defaults = default_triggers();
        let merged = TriggerManager::merge_triggers(loaded, &defaults);
        assert_eq!(merged.len(), 3);
        assert!(merged[0].enabled); // 用户偏好被保留
    }

    #[test]
    fn test_merge_triggers_removes_deprecated_builtins() {
        let deprecated = NotificationTrigger {
            id: "builtin-deprecated-old".to_string(),
            name: "Old Deprecated".to_string(),
            enabled: false,
            content_type: TriggerContentType::ToolResult,
            mode: TriggerMode::ErrorStatus,
            require_error: None,
            tool_name: None,
            match_field: None,
            match_pattern: None,
            token_threshold: None,
            token_type: None,
            ignore_patterns: None,
            is_builtin: Some(true),
            color: None,
            repository_ids: None,
        };
        let mut loaded = default_triggers();
        loaded.push(deprecated);
        let defaults = default_triggers();
        let merged = TriggerManager::merge_triggers(loaded, &defaults);
        assert_eq!(merged.len(), 3);
        assert!(!merged.iter().any(|t| t.id == "builtin-deprecated-old"));
    }

    #[test]
    fn test_merge_triggers_preserves_custom_triggers() {
        let mut loaded = default_triggers();
        loaded.push(custom_trigger("custom-a", "Custom A"));
        loaded.push(custom_trigger("custom-b", "Custom B"));
        let defaults = default_triggers();
        let merged = TriggerManager::merge_triggers(loaded, &defaults);
        assert_eq!(merged.len(), 5);
        assert!(merged.iter().any(|t| t.id == "custom-a"));
        assert!(merged.iter().any(|t| t.id == "custom-b"));
    }

    // =========================================================================
    // infer_mode
    // =========================================================================

    #[test]
    fn test_infer_mode_from_require_error() {
        let trigger = custom_trigger("test", "Test");
        assert_eq!(infer_mode(&trigger), TriggerMode::ErrorStatus);
    }

    #[test]
    fn test_infer_mode_from_match_pattern() {
        let trigger = content_match_trigger("test", "Test", "pattern");
        assert_eq!(infer_mode(&trigger), TriggerMode::ContentMatch);
    }

    #[test]
    fn test_infer_mode_from_token_threshold() {
        let trigger = token_threshold_trigger("test", "Test", 5000);
        assert_eq!(infer_mode(&trigger), TriggerMode::TokenThreshold);
    }

    // =========================================================================
    // set_triggers
    // =========================================================================

    #[test]
    fn test_set_triggers() {
        let mut manager = make_manager(default_triggers());
        let new_triggers = vec![custom_trigger("only-one", "Only")];
        manager.set_triggers(new_triggers);
        assert_eq!(manager.get_all().len(), 1);
        assert_eq!(manager.get_all()[0].id, "only-one");
    }
}
