#[cfg(test)]
mod tests {
    use std::fs;

    use crate::infrastructure::config::ConfigManager;
    use crate::infrastructure::config::defaults;
    use crate::infrastructure::config::path_utils;
    use crate::infrastructure::config_validator::{validate_update_payload, json_merge};

    fn temp_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("claude-devtools-test-{}.json", uuid::Uuid::new_v4()))
    }
    fn cleanup(p: &std::path::PathBuf) { let _ = fs::remove_file(p); }

    #[test]
    fn test_default_config() {
        let c = defaults::default_app_config();
        assert!(c.notifications.enabled && c.notifications.sound_enabled);
        assert_eq!(c.notifications.ignored_regex.len(), 1);
        assert!(c.notifications.snoozed_until.is_none());
        assert_eq!(c.notifications.snooze_minutes, 30);
        assert!(!c.general.launch_at_login && c.general.show_dock_icon);
        assert_eq!(c.general.theme, "dark");
        assert!(c.display.show_timestamps && !c.display.compact_mode);
        assert!(c.sessions.pinned_sessions.is_empty());
        assert!(c.ssh.is_some() && c.http_server.is_none());
    }

    #[tokio::test]
    async fn test_save_and_load_round_trip() {
        let p = temp_path(); cleanup(&p);
        let m1 = ConfigManager::with_path(p.clone()); m1.initialize().await.unwrap();
        m1.pin_session("proj".into(), "sess".into()).await;
        let m2 = ConfigManager::with_path(p.clone()); m2.initialize().await.unwrap();
        assert_eq!(m2.get_config().await.sessions.pinned_sessions["proj"][0].session_id, "sess");
        cleanup(&p);
    }

    #[tokio::test]
    async fn test_add_remove_ignore_regex() {
        let p = temp_path(); let m = ConfigManager::with_path(p.clone());
        assert!(m.add_ignore_regex("test-pat".into()).await.unwrap().notifications.ignored_regex.iter().any(|x| x == "test-pat"));
        assert!(m.add_ignore_regex("test-pat".into()).await.is_err());
        assert!(m.add_ignore_regex("(?P<bad".into()).await.is_err());
        assert!(m.add_ignore_regex("   ".into()).await.is_err());
        assert!(!m.remove_ignore_regex("test-pat".into()).await.notifications.ignored_regex.iter().any(|x| x == "test-pat"));
        cleanup(&p);
    }

    #[tokio::test]
    async fn test_pin_unpin_session() {
        let p = temp_path(); let m = ConfigManager::with_path(p.clone());
        let c = m.pin_session("p".into(), "s1".into()).await;
        assert_eq!(c.sessions.pinned_sessions["p"][0].session_id, "s1");
        let c = m.pin_session("p".into(), "s2".into()).await;
        assert_eq!(c.sessions.pinned_sessions["p"][0].session_id, "s2");
        let c = m.pin_session("p".into(), "s1".into()).await;
        assert_eq!(c.sessions.pinned_sessions["p"].len(), 2);
        let c = m.unpin_session("p".into(), "s2".into()).await;
        assert_eq!(c.sessions.pinned_sessions["p"].len(), 1);
        cleanup(&p);
    }

    #[test]
    fn test_json_merge_deep() {
        let r = json_merge(&serde_json::json!({"a": 1, "b": {"c": 2, "d": 3}}), &serde_json::json!({"a": 10, "b": {"c": 20}}));
        assert_eq!(r["a"], 10); assert_eq!(r["b"]["c"], 20); assert_eq!(r["b"]["d"], 3);
    }

    #[tokio::test]
    async fn test_merge_partial_config() {
        let p = temp_path(); cleanup(&p);
        fs::write(&p, r#"{"general":{"theme":"light"}}"#).unwrap();
        let m = ConfigManager::with_path(p.clone()); m.initialize().await.unwrap();
        let c = m.get_config().await;
        assert_eq!(c.general.theme, "light"); assert!(c.general.show_dock_icon && c.notifications.enabled);
        cleanup(&p);
    }

    #[tokio::test]
    async fn test_initialize_seeds_builtin_triggers() {
        let p = temp_path(); cleanup(&p);
        let m = ConfigManager::with_path(p.clone()); m.initialize().await.unwrap();
        let triggers = m.get_triggers().await;
        assert_eq!(triggers.len(), 3);
        let ids: Vec<&str> = triggers.iter().map(|t| t.id.as_str()).collect();
        assert!(ids.contains(&"builtin-bash-command")); assert!(ids.contains(&"builtin-tool-result-error")); assert!(ids.contains(&"builtin-high-token-usage"));
        for t in &triggers { assert!(!t.enabled); assert_eq!(t.is_builtin, Some(true)); }
        cleanup(&p);
    }

    #[tokio::test]
    async fn test_initialize_preserves_user_triggers_and_merges_builtins() {
        let p = temp_path(); cleanup(&p);
        let config_json = serde_json::json!({
            "notifications": {"triggers": [
                {"id":"my-custom","name":"My Trigger","enabled":true,"contentType":"tool_result","mode":"error_status"},
                {"id":"builtin-tool-result-error","name":"Modified Name","enabled":true,"contentType":"tool_result","mode":"error_status"}
            ]}
        });
        fs::write(&p, serde_json::to_string(&config_json).unwrap()).unwrap();
        let m = ConfigManager::with_path(p.clone()); m.initialize().await.unwrap();
        let triggers = m.get_triggers().await;
        assert_eq!(triggers.len(), 4);
        cleanup(&p);
    }

    // ===== validate_update_payload tests (unchanged from original) =====

    #[test] fn test_validate_rejects_unknown_key_in_notifications() {
        let err = validate_update_payload("notifications", &serde_json::json!({"unknownKey": true})).unwrap_err();
        assert!(err.contains("unknownKey is not supported"));
    }
    #[test] fn test_validate_rejects_unknown_key_in_general() {
        let err = validate_update_payload("general", &serde_json::json!({"unknownKey": "foo"})).unwrap_err();
        assert!(err.contains("not a valid setting"));
    }
    #[test] fn test_validate_rejects_unknown_key_in_display() {
        assert!(validate_update_payload("display", &serde_json::json!({"unknownKey": 42})).unwrap_err().contains("not a valid setting"));
    }
    #[test] fn test_validate_rejects_unknown_key_in_http_server() {
        assert!(validate_update_payload("httpServer", &serde_json::json!({"unknownKey": true})).unwrap_err().contains("not a valid setting"));
    }
    #[test] fn test_validate_rejects_unknown_key_in_ssh() {
        assert!(validate_update_payload("ssh", &serde_json::json!({"unknownKey": true})).unwrap_err().contains("not a valid setting"));
    }
    #[test] fn test_validate_empty_object_passes() {
        for s in &["notifications","general","display","httpServer","ssh"] { assert!(validate_update_payload(s, &serde_json::json!({})).is_ok()); }
    }
    #[test] fn test_validate_non_object_rejects() {
        assert!(validate_update_payload("general", &serde_json::json!("string")).unwrap_err().contains("must be an object"));
    }
    #[test] fn test_validate_notifications_bool_fields() {
        assert!(validate_update_payload("notifications", &serde_json::json!({"enabled": true})).is_ok());
        assert!(validate_update_payload("notifications", &serde_json::json!({"enabled": "true"})).unwrap_err().contains("must be a boolean"));
    }
    #[test] fn test_validate_notifications_snooze_minutes_range() {
        assert!(validate_update_payload("notifications", &serde_json::json!({"snoozeMinutes": 1})).is_ok());
        assert!(validate_update_payload("notifications", &serde_json::json!({"snoozeMinutes": 1441})).unwrap_err().contains("between 1 and 1440"));
    }
    #[test] fn test_validate_notifications_ignored_regex_type() {
        assert!(validate_update_payload("notifications", &serde_json::json!({"ignoredRegex": ["a"]})).is_ok());
        assert!(validate_update_payload("notifications", &serde_json::json!({"ignoredRegex": "x"})).unwrap_err().contains("must be a string[]"));
    }
    #[test] fn test_validate_notifications_snoozed_until() {
        assert!(validate_update_payload("notifications", &serde_json::json!({"snoozedUntil": null})).is_ok());
        assert!(validate_update_payload("notifications", &serde_json::json!({"snoozedUntil": -1})).unwrap_err().contains("non-negative number or null"));
    }
    #[test] fn test_validate_general_theme_enum() {
        assert!(validate_update_payload("general", &serde_json::json!({"theme": "dark"})).is_ok());
        assert!(validate_update_payload("general", &serde_json::json!({"theme": "invalid"})).unwrap_err().contains("must be one of"));
    }
    #[test] fn test_validate_general_claude_root_path() {
        assert!(validate_update_payload("general", &serde_json::json!({"claudeRootPath": "/abs"})).is_ok());
        assert!(validate_update_payload("general", &serde_json::json!({"claudeRootPath": "rel"})).unwrap_err().contains("must be an absolute path"));
    }
    #[test] fn test_validate_http_server_port_range() {
        assert!(validate_update_payload("httpServer", &serde_json::json!({"port": 1024})).is_ok());
        assert!(validate_update_payload("httpServer", &serde_json::json!({"port": 0})).unwrap_err().contains("between 1024 and 65535"));
    }
    #[test] fn test_validate_ssh_auth_method_enum() {
        let profiles = serde_json::json!([{"id":"p1","name":"T","host":"h","port":22,"username":"u","authMethod":"password"}]);
        assert!(validate_update_payload("ssh", &serde_json::json!({"profiles": profiles})).is_ok());
        // Must include all required fields (name/host/port/user) before authMethod is checked
        assert!(validate_update_payload("ssh", &serde_json::json!({"profiles": serde_json::json!([{"id":"p1","name":"T","host":"h","port":22,"username":"u","authMethod":"invalid"}])})).unwrap_err().contains("must be one of"));
    }
    #[test] fn test_validate_sessions_always_passes() {
        assert!(validate_update_payload("sessions", &serde_json::json!({"anything": "goes"})).is_ok());
    }
    #[test] fn test_normalize_root_path_strips_trailing_slash() {
        assert_eq!(path_utils::normalize_claude_root_path("/Users/foo/.claude/"), "/Users/foo/.claude");
    }
    #[test] fn test_normalize_root_path_resolves_dot_segments() {
        assert_eq!(path_utils::normalize_claude_root_path("/Users/foo/../bar/.claude"), "/Users/bar/.claude");
    }
    #[test] fn test_normalize_root_path_preserves_root() {
        assert_eq!(path_utils::normalize_claude_root_path("/"), "/");
    }
    #[test] fn test_normalize_root_path_no_change_needed() {
        assert_eq!(path_utils::normalize_claude_root_path("/Users/foo/.claude"), "/Users/foo/.claude");
    }

    // ===== notifications: ignored_repositories type =====
    #[test] fn test_validate_notifications_ignored_repositories_type() {
        assert!(validate_update_payload("notifications", &serde_json::json!({"ignoredRepositories": ["repo1"]})).is_ok());
        let err = validate_update_payload("notifications", &serde_json::json!({"ignoredRepositories": "not-array"})).unwrap_err();
        assert!(err.contains("must be a string[]"));
    }

    // ===== notifications: triggers validation (id/enabled/contentType/mode required fields) =====
    #[test] fn test_validate_notifications_triggers() {
        let valid_trigger = serde_json::json!({"id":"t1","name":"Test","enabled":true,"contentType":"tool_result","mode":"error_status"});
        assert!(validate_update_payload("notifications", &serde_json::json!({"triggers":[valid_trigger]})).is_ok());
        // trigger without id
        let err = validate_update_payload("notifications", &serde_json::json!({"triggers":[{"name":"Test","enabled":true,"contentType":"tool_result","mode":"error_status"}]})).unwrap_err();
        assert!(err.contains("triggers[0]"));
        // trigger without enabled
        let err = validate_update_payload("notifications", &serde_json::json!({"triggers":[{"id":"t1","name":"Test","contentType":"tool_result","mode":"error_status"}]})).unwrap_err();
        assert!(err.contains("enabled must be a boolean"));
        // trigger without contentType
        let err = validate_update_payload("notifications", &serde_json::json!({"triggers":[{"id":"t1","name":"Test","enabled":true,"mode":"error_status"}]})).unwrap_err();
        assert!(err.contains("contentType must be one of"));
        // invalid contentType
        let err = validate_update_payload("notifications", &serde_json::json!({"triggers":[{"id":"t1","name":"Test","enabled":true,"contentType":"invalid","mode":"error_status"}]})).unwrap_err();
        assert!(err.contains("contentType must be one of"));
        // invalid mode
        let err = validate_update_payload("notifications", &serde_json::json!({"triggers":[{"id":"t1","name":"Test","enabled":true,"contentType":"tool_result","mode":"invalid"}]})).unwrap_err();
        assert!(err.contains("mode must be one of"));
    }

    // ===== general: default_tab enum =====
    #[test] fn test_validate_general_default_tab_enum() {
        assert!(validate_update_payload("general", &serde_json::json!({"defaultTab":"dashboard"})).is_ok());
        assert!(validate_update_payload("general", &serde_json::json!({"defaultTab":"last-session"})).is_ok());
        let err = validate_update_payload("general", &serde_json::json!({"defaultTab":"invalid"})).unwrap_err();
        assert!(err.contains("defaultTab must be one of"));
    }

    // ===== general: bool fields (launchAtLogin/showDockIcon/autoExpandAIGroups/useNativeTitleBar) =====
    #[test] fn test_validate_general_bool_fields() {
        assert!(validate_update_payload("general", &serde_json::json!({"launchAtLogin":true})).is_ok());
        assert!(validate_update_payload("general", &serde_json::json!({"showDockIcon":false})).is_ok());
        assert!(validate_update_payload("general", &serde_json::json!({"autoExpandAIGroups":true})).is_ok());
        assert!(validate_update_payload("general", &serde_json::json!({"useNativeTitleBar":false})).is_ok());
        let err = validate_update_payload("general", &serde_json::json!({"launchAtLogin":"true"})).unwrap_err();
        assert!(err.contains("must be a boolean"));
    }

    // ===== ssh: profile required fields (id/name/host/authMethod) =====
    #[test] fn test_validate_ssh_profile_required_fields() {
        let valid_profiles = serde_json::json!([{"id":"p1","name":"T","host":"h","port":22,"username":"u","authMethod":"password"}]);
        assert!(validate_update_payload("ssh", &serde_json::json!({"profiles":valid_profiles})).is_ok());
        // missing id
        let bad = serde_json::json!([{"name":"T","host":"h","port":22,"username":"u","authMethod":"password"}]);
        let err = validate_update_payload("ssh", &serde_json::json!({"profiles":bad})).unwrap_err();
        assert!(err.contains("id must be a non-empty string"));
        // empty id
        let bad = serde_json::json!([{"id":"","name":"T"}]);
        let err = validate_update_payload("ssh", &serde_json::json!({"profiles":bad})).unwrap_err();
        assert!(err.contains("id must be a non-empty string"));
        // missing name
        let bad = serde_json::json!([{"id":"p1","host":"h","port":22,"username":"u","authMethod":"password"}]);
        let err = validate_update_payload("ssh", &serde_json::json!({"profiles":bad})).unwrap_err();
        assert!(err.contains("name must be a string"));
        // missing host
        let bad = serde_json::json!([{"id":"p1","name":"T","port":22,"username":"u","authMethod":"password"}]);
        let err = validate_update_payload("ssh", &serde_json::json!({"profiles":bad})).unwrap_err();
        assert!(err.contains("host must be a string"));
        // missing authMethod
        let bad = serde_json::json!([{"id":"p1","name":"T","host":"h","port":22,"username":"u"}]);
        let err = validate_update_payload("ssh", &serde_json::json!({"profiles":bad})).unwrap_err();
        assert!(err.contains("authMethod must be one of"));
    }

    // ===== ssh: last_connection types =====
    #[test] fn test_validate_ssh_last_connection_types() {
        assert!(validate_update_payload("ssh", &serde_json::json!({"lastConnection":null})).is_ok());
        assert!(validate_update_payload("ssh", &serde_json::json!({"lastConnection":{}})).is_ok());
        let err = validate_update_payload("ssh", &serde_json::json!({"lastConnection":"string"})).unwrap_err();
        assert!(err.contains("must be an object or null"));
    }

    // ===== display: bool fields =====
    #[test] fn test_validate_display_bool_fields() {
        assert!(validate_update_payload("display", &serde_json::json!({"showTimestamps":true})).is_ok());
        let err = validate_update_payload("display", &serde_json::json!({"showTimestamps":"true"})).unwrap_err();
        assert!(err.contains("must be a boolean"));
    }

    // ===== update_trigger delegation: infer_mode behavior =====

    #[tokio::test]
    async fn test_update_trigger_infer_mode_when_mode_absent() {
        let p = temp_path(); cleanup(&p);
        let m = ConfigManager::with_path(p.clone()); m.initialize().await.unwrap();
        // Add a trigger first
        let trigger = crate::types::config::NotificationTrigger {
            id: "infer-test".to_string(), name: "Infer Test".to_string(), enabled: true,
            content_type: crate::types::config::TriggerContentType::ToolResult,
            mode: crate::types::config::TriggerMode::ErrorStatus,
            require_error: Some(true), tool_name: None, match_field: None,
            match_pattern: None, token_threshold: None, token_type: None,
            ignore_patterns: None, is_builtin: None, color: None, repository_ids: None,
        };
        m.add_trigger(trigger).await.unwrap();

        // Update without mode field — should infer ErrorStatus from require_error=true
        let result = m.update_trigger("infer-test", serde_json::json!({"name": "Updated"})).await.unwrap();
        let updated = result.notifications.triggers.iter().find(|t| t.id == "infer-test").unwrap();
        assert_eq!(updated.name, "Updated");
        // Mode should remain ErrorStatus (inferred from require_error)
        assert_eq!(updated.mode, crate::types::config::TriggerMode::ErrorStatus);
        cleanup(&p);
    }

    #[tokio::test]
    async fn test_update_trigger_preserves_mode_when_explicitly_set() {
        let p = temp_path(); cleanup(&p);
        let m = ConfigManager::with_path(p.clone()); m.initialize().await.unwrap();
        let trigger = crate::types::config::NotificationTrigger {
            id: "preserve-mode".to_string(), name: "Preserve Mode".to_string(), enabled: true,
            content_type: crate::types::config::TriggerContentType::ToolResult,
            mode: crate::types::config::TriggerMode::ContentMatch,
            require_error: None, tool_name: None, match_field: Some("input".to_string()),
            match_pattern: Some(".*".to_string()), token_threshold: None, token_type: None,
            ignore_patterns: None, is_builtin: None, color: None, repository_ids: None,
        };
        m.add_trigger(trigger).await.unwrap();

        // Update WITH explicit mode — should NOT be overridden by infer_mode
        let result = m.update_trigger("preserve-mode", serde_json::json!({
            "mode": "token_threshold",
            "tokenThreshold": 5000,
            "tokenType": "total"
        })).await.unwrap();
        let updated = result.notifications.triggers.iter().find(|t| t.id == "preserve-mode").unwrap();
        assert_eq!(updated.mode, crate::types::config::TriggerMode::TokenThreshold);
        assert_eq!(updated.token_threshold, Some(5000));
        cleanup(&p);
    }

    #[tokio::test]
    async fn test_update_trigger_rejects_invalid_enum_value() {
        let p = temp_path(); cleanup(&p);
        let m = ConfigManager::with_path(p.clone()); m.initialize().await.unwrap();
        let trigger = crate::types::config::NotificationTrigger {
            id: "enum-test".to_string(), name: "Enum Test".to_string(), enabled: true,
            content_type: crate::types::config::TriggerContentType::ToolResult,
            mode: crate::types::config::TriggerMode::ErrorStatus,
            require_error: Some(true), tool_name: None, match_field: None,
            match_pattern: None, token_threshold: None, token_type: None,
            ignore_patterns: None, is_builtin: None, color: None, repository_ids: None,
        };
        m.add_trigger(trigger).await.unwrap();

        // Invalid mode value — should be silently ignored (original value preserved)
        let result = m.update_trigger("enum-test", serde_json::json!({"mode": "invalid_mode"})).await.unwrap();
        let updated = result.notifications.triggers.iter().find(|t| t.id == "enum-test").unwrap();
        assert_eq!(updated.mode, crate::types::config::TriggerMode::ErrorStatus, "invalid mode should be silently ignored");
        cleanup(&p);
    }
}
