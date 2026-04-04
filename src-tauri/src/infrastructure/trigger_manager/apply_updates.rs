//! 触发器字段更新 + 模式推断辅助函数。

use crate::types::config::{NotificationTrigger, TriggerMode};

/// 将 JSON 值中的字段更新应用到触发器，过滤掉 `isBuiltin` 字段。
pub(crate) fn apply_updates(trigger: &mut NotificationTrigger, updates: &serde_json::Value) {
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
    if let Some(repository_ids) = updates.get("repositoryIds").and_then(|v| v.as_array()) {
        trigger.repository_ids = Some(
            repository_ids
                .iter()
                .filter_map(|p| p.as_str().map(String::from))
                .collect(),
        );
    }
    if let Some(token_type) = updates.get("tokenType").and_then(|v| v.as_str()) {
        if let Ok(tt) = serde_json::from_value(serde_json::json!(token_type)) {
            trigger.token_type = Some(tt);
        }
    }
    // 注意: `isBuiltin` 被有意忽略 — 内置状态不可更改。
}

/// 判断是否需要进行模式推断（更新中未包含 mode 字段）。
pub(crate) fn should_infer_mode(updates: &serde_json::Value) -> bool {
    !updates.get("mode").map_or(false, |v| v.is_string())
}

/// 根据触发器属性推断模式，用于向后兼容。
pub(crate) fn infer_mode(trigger: &NotificationTrigger) -> TriggerMode {
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
