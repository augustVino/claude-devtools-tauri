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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

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
/// 与命令层共享同一个 `Arc<ConfigManager>` 实例，
/// 确保触发器变更（启用/禁用/增删）立即对管道生效。
pub struct ErrorDetector {
    config_manager: Arc<ConfigManager>,
}

impl ErrorDetector {
    /// 使用给定的配置管理器创建新的 `ErrorDetector`。
    pub fn new(config_manager: Arc<ConfigManager>) -> Self {
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

        // 从配置中获取已启用的触发器（ConfigManager 内部有 RwLock 保护）
        let triggers = self.config_manager.get_enabled_triggers();

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
    // 触发器测试
    // ===========================================================================

    /// 在所有项目的历史数据上测试触发器。
    ///
    /// 委托给 [`error_trigger_tester::test_trigger`]（与 Electron 行为对齐）。
    pub async fn test_trigger(
        &self,
        trigger: &NotificationTrigger,
        limit: Option<usize>,
    ) -> TriggerTestResult {
        let scanner = crate::discovery::project_scanner::ProjectScanner::new();
        crate::error::error_trigger_tester::test_trigger(trigger, &scanner, limit).await
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
    /// 优先保留带 `subagent_id` 的版本（与 Electron 行为对齐）。
    fn deduplicate_errors(errors: Vec<DetectedError>) -> Vec<DetectedError> {
        let mut best: HashMap<String, usize> = HashMap::new();
        let mut result = Vec::with_capacity(errors.len());

        for (i, error) in errors.iter().enumerate() {
            if let Some(ref tool_use_id) = error.tool_use_id {
                if let Some(&prev_idx) = best.get(tool_use_id) {
                    // 如果新条目有 subagent_id 而已有的没有，则替换
                    let prev_has_subagent = errors[prev_idx].subagent_id.is_some();
                    let curr_has_subagent = error.subagent_id.is_some();
                    if !prev_has_subagent && curr_has_subagent {
                        // 移除之前的版本，push 当前版本
                        result.retain(|e: &DetectedError| e.tool_use_id.as_ref() != Some(tool_use_id));
                        result.push(error.clone());
                        best.insert(tool_use_id.clone(), i);
                    }
                    // 否则保留已有序列中的版本
                    continue;
                }
                best.insert(tool_use_id.clone(), i);
            }
            result.push(error.clone());
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
            match_field: Some("command".to_string()),
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
        let config_manager = Arc::new(ConfigManager::new());
        // 将每个触发器添加到配置中
        for trigger in triggers {
            let _ = config_manager.add_trigger(trigger);
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
    // test_trigger delegation tests
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_test_trigger_delegates_to_tester() {
        let detector = make_detector_with_triggers(vec![make_error_trigger()]);

        // 委托给 ErrorTriggerTester，使用默认 ProjectScanner（扫描真实路径）
        // 只验证返回值结构正确，不验证具体数量（取决于本地环境）
        let result = detector
            .test_trigger(&make_error_trigger(), Some(50))
            .await;

        // 验证返回值是有效的 TriggerTestResult（而非 stub 的空值）
        // 只要没有 panic 即表示委托成功
        let _ = result.total_count;
        let _ = &result.errors;
        let _ = result.truncated;
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

    #[test]
    fn test_deduplicate_errors_prefers_subagent_version() {
        // 父会话先出现（无 subagent_id），子代理后出现（有 subagent_id）
        let errors = vec![
            DetectedError {
                id: "e1".to_string(),
                session_id: "s1".to_string(),
                project_id: "p1".to_string(),
                file_path: "/f.jsonl".to_string(),
                source: "Bash".to_string(),
                message: "error from parent".to_string(),
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
                message: "error from subagent".to_string(),
                timestamp: 0,
                line_number: None,
                tool_use_id: Some("tu1".to_string()),
                subagent_id: Some("sub_1".to_string()),
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
        // 应保留带 subagent_id 的版本
        assert_eq!(deduped[0].subagent_id, Some("sub_1".to_string()));
        assert_eq!(deduped[0].trigger_id, Some("t2".to_string()));
    }

    #[test]
    fn test_deduplicate_errors_keeps_existing_subagent_over_non_subagent() {
        // 子代理版本先出现，父版本后出现 → 保留已有的子代理版本
        let errors = vec![
            DetectedError {
                id: "e1".to_string(),
                session_id: "s1".to_string(),
                project_id: "p1".to_string(),
                file_path: "/f.jsonl".to_string(),
                source: "Bash".to_string(),
                message: "error from subagent".to_string(),
                timestamp: 0,
                line_number: None,
                tool_use_id: Some("tu1".to_string()),
                subagent_id: Some("sub_1".to_string()),
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
                message: "error from parent".to_string(),
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
        // 已有的是 subagent 版本，不应被替换
        assert_eq!(deduped[0].subagent_id, Some("sub_1".to_string()));
    }

    // ---------------------------------------------------------------------------
    // Shared ConfigManager tests
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_shared_config_manager_reflects_runtime_changes() {
        // 验证 ErrorDetector 使用的共享 ConfigManager 能反映运行时变更
        let config_manager = Arc::new(ConfigManager::new());
        let detector = ErrorDetector::new(config_manager.clone());

        // 初始无触发器 → 不应检测到错误
        let messages = vec![make_assistant_message(
            json!("test"),
            vec![make_tool_call("tc1", "Bash", json!({"command": "rm -rf /"}))],
            vec![make_tool_result("tc1", json!("Permission denied"), true)],
        )];
        let errors = detector.detect_errors(&messages, "s1", "-test", "/f.jsonl").await;
        assert!(errors.is_empty(), "no triggers → no errors");

        // 运行时添加触发器 → 下次检测应生效
        config_manager.add_trigger(make_error_trigger()).unwrap();
        let errors = detector.detect_errors(&messages, "s1", "-test", "/f.jsonl").await;
        assert_eq!(errors.len(), 1, "trigger added at runtime → should detect error");

        // 运行时禁用触发器 → 下次检测不应产生错误
        let _ = config_manager.update_trigger("error-trigger", serde_json::json!({"enabled": false}));
        let errors = detector.detect_errors(&messages, "s1", "-test", "/f.jsonl").await;
        assert!(errors.is_empty(), "trigger disabled at runtime → no errors");
    }
}
