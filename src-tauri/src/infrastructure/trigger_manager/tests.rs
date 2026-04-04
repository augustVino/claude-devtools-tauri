#[cfg(test)]
mod tests {
    use crate::types::config::{NotificationTrigger, TriggerContentType, TriggerMode, TriggerTokenType, TriggerValidationResult};
    use crate::infrastructure::trigger_manager::{TriggerManager, default_triggers};
    use crate::infrastructure::trigger_manager::apply_updates as au;
    use std::sync::Arc;

    fn no_op() {}
    fn make_manager(triggers: Vec<NotificationTrigger>) -> TriggerManager {
        TriggerManager::new(triggers, Arc::new(no_op))
    }

    fn custom_trigger(id: &str, name: &str) -> NotificationTrigger {
        NotificationTrigger {
            id: id.to_string(),
            name: name.to_string(),
            enabled: true,
            content_type: crate::types::config::TriggerContentType::ToolResult,
            mode: crate::types::config::TriggerMode::ErrorStatus,
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
            content_type: crate::types::config::TriggerContentType::ToolUse,
            mode: crate::types::config::TriggerMode::ContentMatch,
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
            content_type: crate::types::config::TriggerContentType::ToolResult,
            mode: crate::types::config::TriggerMode::TokenThreshold,
            token_threshold: Some(threshold),
            token_type: Some(crate::types::config::TriggerTokenType::Total),
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

    // ===== default_triggers =====

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
        for t in &triggers { assert_eq!(t.is_builtin, Some(true)); }
    }

    #[test]
    fn test_default_triggers_all_disabled() {
        let triggers = default_triggers();
        for t in &triggers { assert!(!t.enabled); }
    }

    // ===== get_all, get_enabled, get_by_id =====

    #[test]
    fn test_get_all_returns_all() {
        let manager = make_manager(default_triggers());
        assert_eq!(manager.get_all().len(), 3);
    }

    #[test]
    fn test_get_enabled_filters_correctly() {
        let mut triggers = default_triggers(); triggers[0].enabled = true;
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

    // ===== add =====

    #[test]
    fn test_add_valid_trigger() {
        let mut manager = make_manager(default_triggers());
        let result = manager.add(custom_trigger("custom-1", "My Custom Trigger")).unwrap();
        assert_eq!(result.len(), 4);
        assert_eq!(result[3].id, "custom-1");
    }

    #[test]
    fn test_add_duplicate_id_fails() {
        let mut manager = make_manager(default_triggers());
        let result = manager.add(custom_trigger("builtin-bash-command", "Duplicate"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already exists"));
    }

    #[test]
    fn test_add_invalid_trigger_fails() {
        let mut manager = make_manager(default_triggers());
        let trigger = NotificationTrigger {
            id: "bad-trigger".to_string(), name: "".to_string(),
            enabled: true,
            content_type: crate::types::config::TriggerContentType::ToolResult,
            mode: crate::types::config::TriggerMode::ErrorStatus,
            require_error: None, tool_name: None, match_field: None,
            match_pattern: None, token_threshold: None, token_type: None,
            ignore_patterns: None, is_builtin: None, color: None, repository_ids: None,
        };
        assert!(manager.add(trigger).is_err());
    }

    #[test]
    fn test_add_content_match_without_match_field_fails() {
        let mut manager = make_manager(default_triggers());
        let trigger = NotificationTrigger {
            id: "cm-no-field".to_string(), name: "Bad Content Match".to_string(),
            enabled: true,
            content_type: crate::types::config::TriggerContentType::ToolResult,
            mode: crate::types::config::TriggerMode::ContentMatch,
            match_pattern: Some("test".to_string()), match_field: None,
            tool_name: None, require_error: None, token_threshold: None,
            token_type: None, ignore_patterns: None, is_builtin: None,
            color: None, repository_ids: None,
        };
        assert!(manager.add(trigger).is_err());
    }

    // ===== update =====

    #[test]
    fn test_update_trigger_name() {
        let mut manager = make_manager(default_triggers());
        let result = manager.update("builtin-bash-command", serde_json::json!({"name": "Updated Name"})).unwrap();
        assert_eq!(result[0].name, "Updated Name");
    }

    #[test]
    fn test_update_builtin_cannot_change_is_builtin() {
        let mut manager = make_manager(default_triggers());
        let result = manager.update("builtin-bash-command", serde_json::json!({"isBuiltin": false})).unwrap();
        assert_eq!(result[0].is_builtin, Some(true));
    }

    #[test]
    fn test_update_nonexistent_fails() {
        let mut manager = make_manager(default_triggers());
        assert!(manager.update("nonexistent", serde_json::json!({"name": "Nope"})).is_err());
    }

    #[test]
    fn test_update_to_invalid_state_fails() {
        let mut manager = make_manager(default_triggers());
        assert!(manager.update("builtin-bash-command", serde_json::json!({"name": ""})).is_err());
    }

    // ===== remove =====

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
        assert!(manager.remove("builtin-bash-command").is_err());
    }

    #[test]
    fn test_remove_nonexistent_fails() {
        let mut manager = make_manager(default_triggers());
        assert!(manager.remove("nonexistent").is_err());
    }

    // ===== validate (via instance method) =====

    #[test]
    fn test_validate_valid_error_status_trigger() {
        let manager = make_manager(default_triggers());
        assert!(manager.validate(&custom_trigger("t", "V")).valid);
    }

    #[test]
    fn test_validate_valid_content_match_trigger() {
        let manager = make_manager(default_triggers());
        assert!(manager.validate(&content_match_trigger("t", "V", r"\.env$")).valid);
    }

    #[test]
    fn test_validate_valid_token_threshold_trigger() {
        let manager = make_manager(default_triggers());
        assert!(manager.validate(&token_threshold_trigger("t", "V", 5000)).valid);
    }

    #[test]
    fn test_validate_empty_name() {
        let manager = make_manager(default_triggers());
        assert!(!manager.validate(&custom_trigger("t", "")).valid);
    }

    #[test]
    fn test_validate_content_match_missing_match_field() {
        let manager = make_manager(default_triggers());
        let trigger = NotificationTrigger {
            id: "t".to_string(), name: "Bad CM".to_string(), enabled: true,
            content_type: crate::types::config::TriggerContentType::ToolResult,
            mode: crate::types::config::TriggerMode::ContentMatch,
            match_pattern: Some("test".to_string()), match_field: None,
            tool_name: None, require_error: None, token_threshold: None,
            token_type: None, ignore_patterns: None, is_builtin: None,
            color: None, repository_ids: None,
        };
        assert!(!manager.validate(&trigger).valid);
    }

    #[test]
    fn test_validate_content_match_tool_use_without_tool_name_ok() {
        let manager = make_manager(default_triggers());
        let trigger = NotificationTrigger {
            id: "t".to_string(), name: "CM ToolUse Any".to_string(), enabled: true,
            content_type: crate::types::config::TriggerContentType::ToolUse,
            mode: crate::types::config::TriggerMode::ContentMatch,
            match_pattern: Some("test".to_string()), match_field: None,
            tool_name: None, require_error: None, token_threshold: None,
            token_type: None, ignore_patterns: None, is_builtin: None,
            color: None, repository_ids: None,
        };
        assert!(manager.validate(&trigger).valid);
    }

    #[test]
    fn test_validate_token_threshold_zero_passes() {
        let manager = make_manager(default_triggers());
        assert!(manager.validate(&token_threshold_trigger("t", "Z", 0)).valid);
    }

    #[test]
    fn test_validate_token_threshold_missing_type_fails() {
        let manager = make_manager(default_triggers());
        let trigger = NotificationTrigger {
            id: "t".to_string(), name: "Missing Type".to_string(), enabled: true,
            content_type: crate::types::config::TriggerContentType::ToolResult,
            mode: crate::types::config::TriggerMode::TokenThreshold,
            token_threshold: Some(1000), token_type: None,
            tool_name: None, match_field: None, match_pattern: None,
            require_error: None, ignore_patterns: None, is_builtin: None,
            color: None, repository_ids: None,
        };
        assert!(!manager.validate(&trigger).valid);
    }

    #[test]
    fn test_validate_invalid_regex_pattern() {
        let manager = make_manager(default_triggers());
        assert!(!manager.validate(&content_match_trigger("t", "BR", r"(?P<unclosed")).valid);
    }

    #[test]
    fn test_validate_invalid_ignore_pattern() {
        let manager = make_manager(default_triggers());
        let mut trigger = custom_trigger("t", "BI");
        trigger.ignore_patterns = Some(vec![r"(?P<bad".to_string()]);
        assert!(!manager.validate(&trigger).valid);
    }

    // ===== validate_trigger_only (deprecated, 验证行为等价) =====

    #[test]
    fn test_validate_trigger_only_matches_validate() {
        let trigger = custom_trigger("t", "Valid");
        let manager = make_manager(default_triggers());
        // 两者应产生完全相同的结果
        assert_eq!(manager.validate(&trigger), TriggerManager::validate_trigger_only(&trigger));
    }

    // ===== merge_triggers =====

    #[test]
    fn test_merge_triggers_adds_missing_builtins() {
        let merged = TriggerManager::merge_triggers(vec![custom_trigger("c", "C")], &default_triggers());
        assert_eq!(merged.len(), 4);
    }

    #[test]
    fn test_merge_triggers_preserves_existing() {
        let mut loaded = default_triggers(); loaded[0].enabled = true;
        let merged = TriggerManager::merge_triggers(loaded, &default_triggers());
        assert_eq!(merged.len(), 3);
        assert!(merged[0].enabled);
    }

    #[test]
    fn test_merge_triggers_removes_deprecated_builtins() {
        let deprecated = NotificationTrigger {
            id: "builtin-deprecated-old".to_string(), name: "Old Deprecated".to_string(),
            enabled: false,
            content_type: crate::types::config::TriggerContentType::ToolResult,
            mode: crate::types::config::TriggerMode::ErrorStatus,
            require_error: None, tool_name: None, match_field: None,
            match_pattern: None, token_threshold: None, token_type: None,
            ignore_patterns: None, is_builtin: Some(true), color: None, repository_ids: None,
        };
        let mut loaded = default_triggers(); loaded.push(deprecated);
        let merged = TriggerManager::merge_triggers(loaded, &default_triggers());
        assert_eq!(merged.len(), 3);
        assert!(!merged.iter().any(|t| t.id == "builtin-deprecated-old"));
    }

    #[test]
    fn test_merge_triggers_preserves_custom_triggers() {
        let mut loaded = default_triggers();
        loaded.push(custom_trigger("a", "A"));
        loaded.push(custom_trigger("b", "B"));
        let merged = TriggerManager::merge_triggers(loaded, &default_triggers());
        assert_eq!(merged.len(), 5);
    }

    // ===== infer_mode =====

    #[test]
    fn test_infer_mode_from_require_error() {
        assert_eq!(au::infer_mode(&custom_trigger("t", "T")), crate::types::config::TriggerMode::ErrorStatus);
    }

    #[test]
    fn test_infer_mode_from_match_pattern() {
        assert_eq!(au::infer_mode(&content_match_trigger("t", "T", "p")), crate::types::config::TriggerMode::ContentMatch);
    }

    #[test]
    fn test_infer_mode_from_token_threshold() {
        assert_eq!(au::infer_mode(&token_threshold_trigger("t", "T", 5000)), crate::types::config::TriggerMode::TokenThreshold);
    }

    // ===== set_triggers =====

    #[test]
    fn test_set_triggers() {
        let mut manager = make_manager(default_triggers());
        manager.set_triggers(vec![custom_trigger("only-one", "Only")]);
        assert_eq!(manager.get_all().len(), 1);
    }

    // ===== apply_updates — repositoryIds & tokenType =====

    #[test]
    fn test_apply_updates_repository_ids() {
        let mut trigger = default_triggers()[0].clone();
        au::apply_updates(&mut trigger, &serde_json::json!({"repositoryIds": ["repo1", "repo2"]}));
        assert_eq!(trigger.repository_ids, Some(vec!["repo1".to_string(), "repo2".to_string()]));
    }

    #[test]
    fn test_apply_updates_token_type() {
        let mut trigger = default_triggers()[0].clone();
        au::apply_updates(&mut trigger, &serde_json::json!({"tokenType": "output"}));
        assert_eq!(trigger.token_type, Some(crate::types::config::TriggerTokenType::Output));
    }
}
