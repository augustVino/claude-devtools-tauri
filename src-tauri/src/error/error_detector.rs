//! 错误检测器 —— 会话消息中错误检测的主编排器。
//!
//! 协调以下组件：
//! - [`ConfigManager`] — 提供已启用的触发器
//! - [`ErrorTriggerChecker`] — 检查单个触发器是否匹配消息
//! - [`tool_extraction`] — 构建 tool_use/tool_result 映射
//!
//! 检测流程：
//! 1. 从 `ConfigManager` 获取已启用的触发器
//! 2. 预解析仓库 ID（填充缓存）
//! 3. 从消息构建 tool_use/tool_result 映射
//! 4. 对每条消息 x 每个触发器：检查范围、按 content_type 路由、收集错误
//! 5. 按 tool_use_id 去重
//! 6. 返回去重后的列表
//!
//! 从 Electron `src/main/services/error/ErrorDetector.ts` 移植而来。

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use crate::analysis::tool_extraction::{build_tool_result_map, build_tool_use_map};
use crate::error::error_trigger_checker::{
    check_token_threshold_trigger, check_tool_result_trigger, check_tool_use_trigger,
    matches_repository_scope, pre_resolve_repository_ids, RepositoryScopeTarget,
};
use crate::infrastructure::config_manager::ConfigManager;
use crate::types::config::{DetectedError, NotificationTrigger, TriggerContentType, TriggerTestResult};
use crate::types::messages::ParsedMessage;

// =============================================================================
// ErrorDetector（错误检测器）
// =============================================================================

/// 会话消息中错误检测的主编排器。
///
/// 持有 [`ConfigManager`] 的引用用于读取已启用的触发器。
pub struct ErrorDetector {
    config_manager: Arc<RwLock<ConfigManager>>,
}

impl ErrorDetector {
    /// 使用给定的配置管理器创建新的 `ErrorDetector`。
    pub fn new(config_manager: Arc<RwLock<ConfigManager>>) -> Self {
        Self { config_manager }
    }

    // ===========================================================================
    // 主要检测方法
    // ===========================================================================

    /// 使用已启用的触发器检测会话消息中的错误。
    ///
    /// 返回去重后的 [`DetectedError`] 列表。仅处理 `ToolResult` 和 `ToolUse`
    /// 内容类型；`Thinking` 和 `Text` 被跳过（尚未实现）。

    pub async fn detect_errors(
        &self,
        messages: &[ParsedMessage],
        session_id: &str,
        project_id: &str,
        file_path: &str,
    ) -> Vec<DetectedError> {
        let mut errors: Vec<DetectedError> = Vec::new();

        // 从配置中获取已启用的触发器
        let triggers = {
            let config = self
                .config_manager
                .read()
                .expect("config_manager lock poisoned");
            config.get_enabled_triggers()
        };

        if triggers.is_empty() {
            return errors;
        }

        // 预解析此项目的仓库 ID 以填充缓存
        let cwd_hint = messages
            .iter()
            .find(|m| {
                m.cwd
                    .as_ref()
                    .map_or(false, |cwd| !cwd.trim().is_empty())
            })
            .and_then(|m| m.cwd.clone());
        pre_resolve_repository_ids(&[RepositoryScopeTarget {
            project_id: project_id.to_string(),
            cwd_hint,
        }]);

        // 构建映射表用于关联结果与调用、估算 token
        let tool_use_map = build_tool_use_map(messages);
        let tool_result_map = build_tool_result_map(messages);

        // 对每条消息检查每个触发器
        for (i, message) in messages.iter().enumerate() {
            let line_number = (i + 1) as u64; // 从 1 开始的行号

            for trigger in &triggers {
                let trigger_errors = Self::check_trigger(
                    message,
                    trigger,
                    &tool_use_map,
                    &tool_result_map,
                    session_id,
                    project_id,
                    file_path,
                    line_number,
                );

                errors.extend(trigger_errors);
            }
        }

        // 按 tool_use_id 去重
        Self::deduplicate_errors(errors)
    }

    // ===========================================================================
    // 触发器测试（桩实现）
    // ===========================================================================

    /// 在所有项目的历史数据上测试触发器。
    ///
    /// 这是一个桩实现，将在 Task 11 中通过
    /// `ErrorTriggerTester` 实现。
    pub async fn test_trigger(
        &self,
        _trigger: &NotificationTrigger,
        _limit: Option<usize>,
    ) -> TriggerTestResult {
        // 将在 Task 11 中通过 ErrorTriggerTester 实现
        TriggerTestResult {
            total_count: 0,
            errors: vec![],
            truncated: None,
        }
    }

    // ===========================================================================
    // 私有方法：触发器路由
    // ===========================================================================

    /// 根据触发器配置将消息路由到相应的触发器检查器。
    /// 返回零个或多个 `DetectedError` 实例。
    fn check_trigger(
        message: &ParsedMessage,
        trigger: &NotificationTrigger,
        tool_use_map: &std::collections::HashMap<
            String,
            crate::analysis::tool_extraction::ToolUseInfo,
        >,
        tool_result_map: &std::collections::HashMap<
            String,
            crate::analysis::tool_extraction::ToolResultInfo,
        >,
        session_id: &str,
        project_id: &str,
        file_path: &str,
        line_number: u64,
    ) -> Vec<DetectedError> {
        // 首先检查仓库范围
        if !matches_repository_scope(project_id, trigger.repository_ids.as_deref()) {
            return vec![];
        }

        // 处理 token_threshold 模式 —— 逐个检查每个 tool_use
        if trigger.mode == crate::types::config::TriggerMode::TokenThreshold {
            return check_token_threshold_trigger(
                message,
                trigger,
                tool_result_map,
                session_id,
                project_id,
                file_path,
                line_number,
            );
        }

        // 处理 tool_result 触发器
        if trigger.content_type == TriggerContentType::ToolResult {
            if let Some(error) = check_tool_result_trigger(
                message,
                trigger,
                tool_use_map,
                session_id,
                project_id,
                file_path,
                line_number,
            ) {
                return vec![error];
            }
            return vec![];
        }

        // 处理 tool_use 触发器
        if trigger.content_type == TriggerContentType::ToolUse {
            if let Some(error) = check_tool_use_trigger(
                message,
                trigger,
                session_id,
                project_id,
                file_path,
                line_number,
            ) {
                return vec![error];
            }
            return vec![];
        }

        // Thinking 和 Text 内容类型尚未实现
        vec![]
    }

    // ===========================================================================
    // 私有方法：去重
    // ===========================================================================

    /// 按 `tool_use_id` 去重错误。当多个触发器检测到同一个 tool_use 时，
    /// 仅保留首次检测的结果。
    fn deduplicate_errors(errors: Vec<DetectedError>) -> Vec<DetectedError> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut result = Vec::with_capacity(errors.len());

        for error in errors {
            if let Some(ref tool_use_id) = error.tool_use_id {
                if seen.contains(tool_use_id) {
                    continue;
                }
                seen.insert(tool_use_id.clone());
            }
            result.push(error);
        }

        result
    }
}

// =============================================================================
// 测试
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::config::{TriggerMode, TriggerTokenType};
    use crate::types::domain::MessageType;
    use crate::types::messages::{ToolCall, ToolResult};
    use serde_json::json;

    // ---------------------------------------------------------------------------
    // 辅助函数
    // ---------------------------------------------------------------------------

    /// 创建默认的错误触发器（require_error = true，ToolResult 内容类型）。
    fn make_error_trigger() -> NotificationTrigger {
        NotificationTrigger {
            id: "error-trigger".to_string(),
            name: "Error Trigger".to_string(),
            enabled: true,
            content_type: TriggerContentType::ToolResult,
            tool_name: None,
            is_builtin: None,
            ignore_patterns: None,
            mode: TriggerMode::ErrorStatus,
            require_error: Some(true),
            match_field: None,
            match_pattern: None,
            token_threshold: None,
            token_type: None,
            repository_ids: None,
            color: Some("red".to_string()),
        }
    }

    /// 创建一个内容类型不匹配的触发器（Thinking）。
    fn make_thinking_trigger() -> NotificationTrigger {
        NotificationTrigger {
            id: "thinking-trigger".to_string(),
            name: "Thinking Trigger".to_string(),
            enabled: true,
            content_type: TriggerContentType::Thinking,
            tool_name: None,
            is_builtin: None,
            ignore_patterns: None,
            mode: TriggerMode::ErrorStatus,
            require_error: None,
            match_field: None,
            match_pattern: None,
            token_threshold: None,
            token_type: None,
            repository_ids: None,
            color: None,
        }
    }

    /// 创建一个匹配特定工具名称和模式的 tool_use 触发器。
    fn make_tool_use_trigger(tool_name: &str, pattern: &str) -> NotificationTrigger {
        NotificationTrigger {
            id: "tool-use-trigger".to_string(),
            name: "Tool Use Trigger".to_string(),
            enabled: true,
            content_type: TriggerContentType::ToolUse,
            tool_name: Some(tool_name.to_string()),
            is_builtin: None,
            ignore_patterns: None,
            mode: TriggerMode::ContentMatch,
            require_error: None,
            match_field: None,
            match_pattern: Some(pattern.to_string()),
            token_threshold: None,
            token_type: None,
            repository_ids: None,
            color: None,
        }
    }

    fn make_assistant_message(
        content: serde_json::Value,
        tool_calls: Vec<ToolCall>,
        tool_results: Vec<ToolResult>,
    ) -> ParsedMessage {
        ParsedMessage {
            uuid: "test-uuid".to_string(),
            parent_uuid: None,
            message_type: MessageType::Assistant,
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
            role: Some("assistant".to_string()),
            content,
            usage: None,
            model: None,
            cwd: Some("/home/user/project".to_string()),
            git_branch: None,
            agent_id: None,
            is_sidechain: false,
            is_meta: false,
            user_type: None,
            tool_calls,
            tool_results,
            source_tool_use_id: None,
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            is_compact_summary: None,
            request_id: None,
        }
    }

    fn make_tool_call(id: &str, name: &str, input: serde_json::Value) -> ToolCall {
        ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            input,
            is_task: false,
            task_description: None,
            task_subagent_type: None,
        }
    }

    fn make_tool_result(id: &str, content: serde_json::Value, is_error: bool) -> ToolResult {
        ToolResult {
            tool_use_id: id.to_string(),
            content,
            is_error,
        }
    }

    /// 创建一个包含给定触发器的 `ErrorDetector`（使用真实的 `ConfigManager`）。
    fn make_detector_with_triggers(triggers: Vec<NotificationTrigger>) -> ErrorDetector {
        let config_manager = Arc::new(RwLock::new(ConfigManager::new()));
        // 将每个触发器添加到配置中
        {
            let config = config_manager
                .read()
                .expect("config_manager lock poisoned");
            for trigger in triggers {
                let _ = config.add_trigger(trigger);
            }
        }
        ErrorDetector::new(config_manager)
    }

    // ---------------------------------------------------------------------------
    // detect_errors tests
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_detect_errors_no_triggers_returns_empty() {
        let detector = make_detector_with_triggers(vec![]);
        let messages = vec![make_assistant_message(
            json!("test"),
            vec![make_tool_call(
                "tc1",
                "Bash",
                json!({"command": "npm build"}),
            )],
            vec![make_tool_result("tc1", json!("Build failed"), true)],
        )];

        let errors = detector
            .detect_errors(&messages, "session-1", "-Users-test", "/path.jsonl")
            .await;

        assert!(errors.is_empty());
    }

    #[tokio::test]
    async fn test_detect_errors_matching_trigger_returns_errors() {
        let detector = make_detector_with_triggers(vec![make_error_trigger()]);

        let messages = vec![make_assistant_message(
            json!("test"),
            vec![make_tool_call(
                "tc1",
                "Bash",
                json!({"command": "npm build"}),
            )],
            vec![make_tool_result("tc1", json!("Build failed: error E0425"), true)],
        )];

        let errors = detector
            .detect_errors(&messages, "session-1", "-Users-test", "/path.jsonl")
            .await;

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].source, "Bash");
        assert_eq!(errors[0].message, "Build failed: error E0425");
        assert_eq!(errors[0].trigger_id, Some("error-trigger".to_string()));
    }

    #[tokio::test]
    async fn test_detect_errors_mismatched_content_type_skips_trigger() {
        // Thinking 内容类型应被跳过（尚未实现）
        let detector = make_detector_with_triggers(vec![make_thinking_trigger()]);

        let messages = vec![make_assistant_message(
            json!("test"),
            vec![make_tool_call(
                "tc1",
                "Bash",
                json!({"command": "npm build"}),
            )],
            vec![make_tool_result("tc1", json!("Build failed"), true)],
        )];

        let errors = detector
            .detect_errors(&messages, "session-1", "-Users-test", "/path.jsonl")
            .await;

        assert!(errors.is_empty());
    }

    #[tokio::test]
    async fn test_detect_errors_no_error_in_result_no_match() {
        let detector = make_detector_with_triggers(vec![make_error_trigger()]);

        let messages = vec![make_assistant_message(
            json!("test"),
            vec![make_tool_call(
                "tc1",
                "Bash",
                json!({"command": "npm build"}),
            )],
            vec![make_tool_result("tc1", json!("Build succeeded"), false)],
        )];

        let errors = detector
            .detect_errors(&messages, "session-1", "-Users-test", "/path.jsonl")
            .await;

        // require_error=true 但结果不是错误
        assert!(errors.is_empty());
    }

    #[tokio::test]
    async fn test_detect_errors_tool_use_trigger() {
        let detector = make_detector_with_triggers(vec![make_tool_use_trigger("Bash", "rm")]);

        let content = json!([{
            "type": "tool_use",
            "id": "tu1",
            "name": "Bash",
            "input": {"command": "rm -rf /tmp/test"}
        }]);

        let messages = vec![make_assistant_message(content, vec![], vec![])];

        let errors = detector
            .detect_errors(&messages, "session-1", "-Users-test", "/path.jsonl")
            .await;

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].source, "Bash");
    }

    #[tokio::test]
    async fn test_detect_errors_deduplicates_by_tool_use_id() {
        // 两个触发器都匹配同一个 tool_use，去重后应只有一个错误

        let trigger1 = NotificationTrigger {
            id: "trigger-1".to_string(),
            name: "Error Trigger 1".to_string(),
            ..make_error_trigger()
        };
        let trigger2 = NotificationTrigger {
            id: "trigger-2".to_string(),
            name: "Error Trigger 2".to_string(),
            ..make_error_trigger()
        };

        let detector = make_detector_with_triggers(vec![trigger1, trigger2]);

        let messages = vec![make_assistant_message(
            json!("test"),
            vec![make_tool_call(
                "tc1",
                "Bash",
                json!({"command": "npm build"}),
            )],
            vec![make_tool_result("tc1", json!("Build failed"), true)],
        )];

        let errors = detector
            .detect_errors(&messages, "session-1", "-Users-test", "/path.jsonl")
            .await;

        // 两个触发器匹配同一个 tool_use_id，因此只保留一个
        assert_eq!(errors.len(), 1);
    }

    #[tokio::test]
    async fn test_detect_errors_empty_messages() {
        let detector = make_detector_with_triggers(vec![make_error_trigger()]);

        let errors = detector
            .detect_errors(&[], "session-1", "-Users-test", "/path.jsonl")
            .await;

        assert!(errors.is_empty());
    }

    // ---------------------------------------------------------------------------
    // test_trigger stub tests
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_test_trigger_returns_empty_stub() {
        let detector = make_detector_with_triggers(vec![make_error_trigger()]);

        let result = detector
            .test_trigger(&make_error_trigger(), Some(50))
            .await;

        assert_eq!(result.total_count, 0);
        assert!(result.errors.is_empty());
        assert_eq!(result.truncated, None);
    }

    // ---------------------------------------------------------------------------
    // deduplicate_errors tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_deduplicate_errors_no_duplicates() {
        let errors = vec![
            DetectedError {
                id: "e1".to_string(),
                session_id: "s1".to_string(),
                project_id: "p1".to_string(),
                file_path: "/f.jsonl".to_string(),
                source: "Bash".to_string(),
                message: "error 1".to_string(),
                timestamp: 0,
                line_number: None,
                tool_use_id: Some("tu1".to_string()),
                subagent_id: None,
                trigger_color: None,
                trigger_id: None,
                trigger_name: None,
                context: crate::types::config::ErrorContext {
                    project_name: "proj".to_string(),
                    cwd: None,
                },
            },
            DetectedError {
                id: "e2".to_string(),
                session_id: "s1".to_string(),
                project_id: "p1".to_string(),
                file_path: "/f.jsonl".to_string(),
                source: "Read".to_string(),
                message: "error 2".to_string(),
                timestamp: 0,
                line_number: None,
                tool_use_id: Some("tu2".to_string()),
                subagent_id: None,
                trigger_color: None,
                trigger_id: None,
                trigger_name: None,
                context: crate::types::config::ErrorContext {
                    project_name: "proj".to_string(),
                    cwd: None,
                },
            },
        ];

        let deduped = ErrorDetector::deduplicate_errors(errors);
        assert_eq!(deduped.len(), 2);
    }

    #[test]
    fn test_deduplicate_errors_removes_duplicates() {
        let errors = vec![
            DetectedError {
                id: "e1".to_string(),
                session_id: "s1".to_string(),
                project_id: "p1".to_string(),
                file_path: "/f.jsonl".to_string(),
                source: "Bash".to_string(),
                message: "error 1".to_string(),
                timestamp: 0,
                line_number: None,
                tool_use_id: Some("tu1".to_string()),
                subagent_id: None,
                trigger_color: None,
                trigger_id: Some("t1".to_string()),
                trigger_name: None,
                context: crate::types::config::ErrorContext {
                    project_name: "proj".to_string(),
                    cwd: None,
                },
            },
            DetectedError {
                id: "e2".to_string(),
                session_id: "s1".to_string(),
                project_id: "p1".to_string(),
                file_path: "/f.jsonl".to_string(),
                source: "Bash".to_string(),
                message: "error 1".to_string(),
                timestamp: 0,
                line_number: None,
                tool_use_id: Some("tu1".to_string()),
                subagent_id: None,
                trigger_color: None,
                trigger_id: Some("t2".to_string()),
                trigger_name: None,
                context: crate::types::config::ErrorContext {
                    project_name: "proj".to_string(),
                    cwd: None,
                },
            },
        ];

        let deduped = ErrorDetector::deduplicate_errors(errors);
        assert_eq!(deduped.len(), 1);
        assert_eq!(deduped[0].trigger_id, Some("t1".to_string()));
    }

    #[test]
    fn test_deduplicate_errors_keeps_errors_without_tool_use_id() {
        let errors = vec![
            DetectedError {
                id: "e1".to_string(),
                session_id: "s1".to_string(),
                project_id: "p1".to_string(),
                file_path: "/f.jsonl".to_string(),
                source: "Bash".to_string(),
                message: "error 1".to_string(),
                timestamp: 0,
                line_number: None,
                tool_use_id: None,
                subagent_id: None,
                trigger_color: None,
                trigger_id: None,
                trigger_name: None,
                context: crate::types::config::ErrorContext {
                    project_name: "proj".to_string(),
                    cwd: None,
                },
            },
            DetectedError {
                id: "e2".to_string(),
                session_id: "s1".to_string(),
                project_id: "p1".to_string(),
                file_path: "/f.jsonl".to_string(),
                source: "Read".to_string(),
                message: "error 2".to_string(),
                timestamp: 0,
                line_number: None,
                tool_use_id: None,
                subagent_id: None,
                trigger_color: None,
                trigger_id: None,
                trigger_name: None,
                context: crate::types::config::ErrorContext {
                    project_name: "proj".to_string(),
                    cwd: None,
                },
            },
        ];

        let deduped = ErrorDetector::deduplicate_errors(errors);
        // 没有 tool_use_id 的错误全部保留（无去重键）
        assert_eq!(deduped.len(), 2);
    }

    #[test]
    fn test_deduplicate_errors_empty() {
        let deduped = ErrorDetector::deduplicate_errors(vec![]);
        assert!(deduped.is_empty());
    }
}
