//! Tests for NotificationManager.

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::types::config::{
        DetectedError, ErrorContext, StoredNotification, AppConfig, DisplayConfig,
        GeneralConfig, GetNotificationsOptions, NotificationConfig, SessionConfig,
    };
    use crate::infrastructure::ConfigManager;

    use super::super::{NotificationManager, MAX_NOTIFICATIONS};
    use super::super::emission::{now_millis, truncate_str};
    use super::super::filtering::NotificationManagerExt;

    /// 创建用于测试的默认 AppConfig。
    fn default_app_config() -> AppConfig {
        AppConfig {
            notifications: NotificationConfig {
                enabled: true,
                sound_enabled: true,
                ignored_regex: vec![],
                ignored_repositories: vec![],
                snoozed_until: None,
                snooze_minutes: 30,
                include_subagent_errors: true,
                triggers: vec![],
            },
            general: GeneralConfig {
                launch_at_login: false,
                show_dock_icon: true,
                theme: "dark".to_string(),
                default_tab: "dashboard".to_string(),
                claude_root_path: None,
                auto_expand_ai_groups: false,
                use_native_title_bar: false,
            },
            display: DisplayConfig {
                show_timestamps: true,
                compact_mode: false,
                syntax_highlighting: true,
            },
            sessions: SessionConfig {
                pinned_sessions: HashMap::new(),
                hidden_sessions: HashMap::new(),
            },
            ssh: None,
            http_server: None,
        }
    }

    /// 创建用于测试的 DetectedError。
    fn make_error(id: &str, project_id: &str, message: &str) -> DetectedError {
        DetectedError {
            id: id.to_string(),
            session_id: "test-session".to_string(),
            project_id: project_id.to_string(),
            file_path: "/tmp/test.rs".to_string(),
            source: "Bash".to_string(),
            message: message.to_string(),
            timestamp: now_millis(),
            line_number: Some(42),
            tool_use_id: None,
            subagent_id: None,
            trigger_color: None,
            trigger_id: None,
            trigger_name: None,
            context: ErrorContext {
                project_name: "TestProject".to_string(),
                cwd: None,
            },
        }
    }

    /// 创建带 tool_use_id 的 DetectedError（用于测试）。
    fn make_error_with_tool(
        id: &str,
        project_id: &str,
        message: &str,
        tool_use_id: &str,
        subagent_id: Option<&str>,
    ) -> DetectedError {
        let mut error = make_error(id, project_id, message);
        error.tool_use_id = Some(tool_use_id.to_string());
        error.subagent_id = subagent_id.map(String::from);
        error
    }

    /// 创建用于测试的 NotificationManager。
    async fn make_manager() -> NotificationManager {
        let cm = Arc::new(ConfigManager::with_path(
            std::env::temp_dir().join(format!(
                "claude-devtools-test-config-{}.json",
                uuid::Uuid::new_v4()
            )),
        ));
        NotificationManager::new_for_test(cm)
    }

    // --- initialize ---

    #[tokio::test]
    async fn test_initialize_missing_file() {
        let mut mgr = make_manager().await;
        // 缺失文件不应 panic，通知列表应为空
        mgr.initialize().await;
        assert_eq!(
            mgr.notifications.read().unwrap().len(),
            0
        );
    }

    #[tokio::test]
    async fn test_initialize_loads_from_file() {
        let mut mgr = make_manager().await;
        let path = mgr.persistence_path().clone();

        // 向文件写入一条通知
        let stored = StoredNotification {
            error: make_error("n1", "proj-1", "test error"),
            is_read: false,
            created_at: 1000,
        };
        let data = serde_json::to_string_pretty(&vec![stored]).unwrap();
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        tokio::fs::write(&path, &data).await.unwrap();

        mgr.initialize().await;
        assert_eq!(mgr.notifications.read().unwrap().len(), 1);

        // 清理
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn test_initialize_invalid_json() {
        let mut mgr = make_manager().await;
        let path = mgr.persistence_path().clone();

        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        tokio::fs::write(&path, "not valid json").await.unwrap();

        mgr.initialize().await;
        assert_eq!(mgr.notifications.read().unwrap().len(), 0);

        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn test_initialize_prunes_excess() {
        let mut mgr = make_manager().await;
        let path = mgr.persistence_path().clone();

        // 创建超过 MAX_NOTIFICATIONS 的条目
        let many: Vec<StoredNotification> = (0..150)
            .map(|i| StoredNotification {
                error: make_error(&format!("n{i}"), "proj", &format!("error {i}")),
                is_read: false,
                created_at: i as u64,
            })
            .collect();

        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        tokio::fs::write(&path, serde_json::to_string(&many).unwrap())
            .await
            .unwrap();

        mgr.initialize().await;
        let count = mgr.notifications.read().unwrap().len();
        assert_eq!(count, MAX_NOTIFICATIONS);

        let _ = tokio::fs::remove_file(&path).await;
    }

    // --- add_error ---

    #[tokio::test]
    async fn test_add_error_stores_notification() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        let error = make_error("e1", "proj-1", "something broke");
        let result = mgr.add_error(error.clone()).await;

        assert!(result.is_some());
        let stored = result.unwrap();
        assert_eq!(stored.error.id, "e1");
        assert!(!stored.is_read);

        let count = mgr.notifications.read().unwrap().len();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_add_error_dedup_same_tool_use_id() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        let error1 = make_error_with_tool("e1", "proj-1", "error msg", "tool-1", None);
        let result1 = mgr.add_error(error1).await;
        assert!(result1.is_some());

        // 相同 toolUseId，无子代理 — 应被去重（跳过）
        let error2 = make_error_with_tool("e2", "proj-1", "different msg", "tool-1", None);
        let result2 = mgr.add_error(error2).await;
        assert!(result2.is_none());

        assert_eq!(mgr.notifications.read().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_add_error_dedup_replace_with_subagent() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        // 第一个: 无子代理
        let error1 = make_error_with_tool("e1", "proj-1", "error msg", "tool-1", None);
        let result1 = mgr.add_error(error1).await;
        assert!(result1.is_some());

        // 第二个: 有子代理 — 应替换
        let error2 =
            make_error_with_tool("e2", "proj-1", "error msg v2", "tool-1", Some("sub-1"));
        let result2 = mgr.add_error(error2).await;
        assert!(result2.is_some());
        assert_eq!(result2.unwrap().error.subagent_id, Some("sub-1".to_string()));

        assert_eq!(mgr.notifications.read().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_add_error_dedup_keep_existing_subagent() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        // 第一个: 有子代理
        let error1 =
            make_error_with_tool("e1", "proj-1", "error msg", "tool-1", Some("sub-1"));
        let result1 = mgr.add_error(error1).await;
        assert!(result1.is_some());

        // 第二个: 无子代理 — 已有条目更优，跳过
        let error2 = make_error_with_tool("e2", "proj-1", "error msg v2", "tool-1", None);
        let result2 = mgr.add_error(error2).await;
        assert!(result2.is_none());

        assert_eq!(mgr.notifications.read().unwrap().len(), 1);
        assert_eq!(
            mgr.notifications.read().unwrap()[0].error.subagent_id,
            Some("sub-1".to_string())
        );
    }

    #[tokio::test]
    async fn test_add_error_no_tool_use_id_no_dedup() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        let error1 = make_error("e1", "proj-1", "error msg");
        let error2 = make_error("e2", "proj-1", "error msg");
        assert!(mgr.add_error(error1).await.is_some());
        assert!(mgr.add_error(error2).await.is_some());

        assert_eq!(mgr.notifications.read().unwrap().len(), 2);
    }

    // --- throttling ---

    #[tokio::test]
    async fn test_add_error_throttle_native_notification() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        // 快速添加两次相同错误 — 两者都应存储（无去重，ID 不同）
        // 但第二次的原生通知应被节流。
        // 由于无法在单元测试中直接测试原生通知，
        // 因此验证节流映射的状态。
        let error = make_error("e1", "proj-1", "throttle test");
        mgr.add_error(error.clone()).await;

        // 检查节流映射中是否有条目
        let hash = NotificationManager::generate_error_hash(&error);
        let throttle_map = mgr.throttle_map.lock().unwrap();
        assert!(throttle_map.contains_key(&hash));
    }

    // --- get_notifications ---

    #[tokio::test]
    async fn test_get_notifications_default_pagination() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        for i in 0..5 {
            let error = make_error(&format!("e{i}"), "proj", &format!("msg {i}"));
            mgr.add_error(error).await;
        }

        let result = mgr
            .get_notifications(GetNotificationsOptions {
                limit: None,
                offset: None,
            })
            .await;

        assert_eq!(result.notifications.len(), 5);
        assert_eq!(result.total, 5);
        assert_eq!(result.total_count, 5);
        assert_eq!(result.unread_count, 5);
        assert!(!result.has_more);
    }

    #[tokio::test]
    async fn test_get_notifications_with_limit_and_offset() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        for i in 0..10 {
            let error = make_error(&format!("e{i}"), "proj", &format!("msg {i}"));
            mgr.add_error(error).await;
        }

        // 最新的在前，所以 e9 排第一
        let page1 = mgr
            .get_notifications(GetNotificationsOptions {
                limit: Some(3),
                offset: Some(0),
            })
            .await;
        assert_eq!(page1.notifications.len(), 3);
        assert_eq!(page1.notifications[0].error.id, "e9");
        assert_eq!(page1.notifications[2].error.id, "e7");
        assert!(page1.has_more);

        let page2 = mgr
            .get_notifications(GetNotificationsOptions {
                limit: Some(3),
                offset: Some(3),
            })
            .await;
        assert_eq!(page2.notifications.len(), 3);
        assert_eq!(page2.notifications[0].error.id, "e6");
        assert!(page2.has_more);

        let page_last = mgr
            .get_notifications(GetNotificationsOptions {
                limit: Some(3),
                offset: Some(9),
            })
            .await;
        assert_eq!(page_last.notifications.len(), 1);
        assert!(!page_last.has_more);
    }

    // --- mark_read ---

    #[tokio::test]
    async fn test_mark_read() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        let error = make_error("e1", "proj", "msg");
        mgr.add_error(error).await;

        assert_eq!(mgr.get_unread_count().await, 1);

        let result = mgr.mark_read("e1").await;
        assert!(result);
        assert_eq!(mgr.get_unread_count().await, 0);

        // 标记已读的条目仍视为"找到"
        let result2 = mgr.mark_read("e1").await;
        assert!(result2);
    }

    #[tokio::test]
    async fn test_mark_read_not_found() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        let result = mgr.mark_read("nonexistent").await;
        assert!(!result);
    }

    // --- mark_all_read ---

    #[tokio::test]
    async fn test_mark_all_read() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        for i in 0..3 {
            let error = make_error(&format!("e{i}"), "proj", &format!("msg {i}"));
            mgr.add_error(error).await;
        }

        assert_eq!(mgr.get_unread_count().await, 3);

        let result = mgr.mark_all_read().await;
        assert!(result);
        assert_eq!(mgr.get_unread_count().await, 0);
    }

    // --- clear_all ---

    #[tokio::test]
    async fn test_clear_all() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        for i in 0..3 {
            let error = make_error(&format!("e{i}"), "proj", &format!("msg {i}"));
            mgr.add_error(error).await;
        }

        assert_eq!(mgr.notifications.read().unwrap().len(), 3);

        let result = mgr.clear_all().await;
        assert!(result);
        assert_eq!(mgr.notifications.read().unwrap().len(), 0);
    }

    // --- delete_notification ---

    #[tokio::test]
    async fn test_delete_notification() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        mgr.add_error(make_error("e1", "proj", "msg1")).await;
        mgr.add_error(make_error("e2", "proj", "msg2")).await;

        assert_eq!(mgr.notifications.read().unwrap().len(), 2);

        let result = mgr.delete_notification("e1").await;
        assert!(result);
        assert_eq!(mgr.notifications.read().unwrap().len(), 1);
        assert_eq!(mgr.notifications.read().unwrap()[0].error.id, "e2");
    }

    #[tokio::test]
    async fn test_delete_notification_not_found() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        let result = mgr.delete_notification("nonexistent").await;
        assert!(!result);
    }

    // --- get_unread_count ---

    #[tokio::test]
    async fn test_get_unread_count() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        assert_eq!(mgr.get_unread_count().await, 0);

        mgr.add_error(make_error("e1", "proj", "msg1")).await;
        assert_eq!(mgr.get_unread_count().await, 1);

        mgr.mark_read("e1").await;
        assert_eq!(mgr.get_unread_count().await, 0);
    }

    // --- get_stats ---

    #[tokio::test]
    async fn test_get_stats() {
        let mut mgr = make_manager().await;
        mgr.initialize().await;

        let mut err1 = make_error("e1", "proj-a", "error from bash");
        err1.source = "Bash".to_string();
        err1.context.project_name = "ProjectA".to_string();

        let mut err2 = make_error("e2", "proj-b", "error from edit");
        err2.source = "Edit".to_string();
        err2.context.project_name = "ProjectB".to_string();

        let mut err3 = make_error("e3", "proj-a", "another bash error");
        err3.source = "Bash".to_string();
        err3.context.project_name = "ProjectA".to_string();

        mgr.add_error(err1).await;
        mgr.add_error(err2).await;
        mgr.add_error(err3).await;

        let stats = mgr.get_stats();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.unread, 3);
        assert_eq!(stats.by_project.get("ProjectA"), Some(&2));
        assert_eq!(stats.by_project.get("ProjectB"), Some(&1));
        assert_eq!(stats.by_source.get("Bash"), Some(&2));
        assert_eq!(stats.by_source.get("Edit"), Some(&1));
    }

    // --- matches_ignored_regex ---

    #[tokio::test]
    async fn test_ignored_regex_filtering() {
        let cm = Arc::new(ConfigManager::with_path(
            std::env::temp_dir().join(format!(
                "claude-devtools-test-config-{}.json",
                uuid::Uuid::new_v4()
            )),
        ));
        cm.add_ignore_regex("permission denied".to_string()).await.unwrap();

        let mut mgr = NotificationManager::new_for_test(cm);
        mgr.initialize().await;

        let error = make_error("e1", "proj", "Permission denied: access forbidden");
        // 错误仍应被存储（存储不受条件限制）
        let result = mgr.add_error(error).await;
        assert!(result.is_some());

        // 但 should_notify 应因忽略正则返回 false
        //（无法直接测试 should_notify 因为它是私有方法，
        // 但可以验证错误虽被过滤但仍已存储）
        assert_eq!(mgr.notifications.read().unwrap().len(), 1);
    }

    // --- 截断辅助函数 ---

    #[test]
    fn test_truncate_str_short() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_str_exact() {
        assert_eq!(truncate_str("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_str_long() {
        let result = truncate_str("hello world", 8);
        assert!(result.starts_with("hello wo"));
        assert!(result.ends_with("..."));
    }
}
