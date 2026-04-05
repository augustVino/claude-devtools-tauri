//! 触发器验证逻辑（含 ReDoS 防护）。

use crate::types::config::{NotificationTrigger, TriggerContentType, TriggerMode};
use crate::utils::regex_validation::validate_regex_pattern;

/// 内部验证函数 — 合并了原先 `validate()` 和 `validate_trigger_only()` 的完全重复逻辑。
///
/// 两个原始方法逐行对比完全等价：`&self` 参数完全不读取 self 状态（无 triggers/on_save 访问），
/// 是典型的"误用实例方法"。合并后由 mod.rs 中的两个公开 API 统一委托。
pub(crate) fn validate_trigger_internal(trigger: &NotificationTrigger) -> Vec<String> {
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

    errors
}
