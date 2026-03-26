//! 错误触发器检查器 —— 检查不同类型的触发器是否匹配消息。
//!
//! 提供以下功能：
//! - 检查 tool_result 触发器（error_status + content_match 模式）
//! - 检查 tool_use 触发器（基于工具输入内容的匹配）
//! - 检查 token_threshold 触发器（按 tool_use 的 token 计数）
//! - 验证项目范围（仓库过滤）
//!
//! 从 Electron `src/main/services/error/ErrorTriggerChecker.ts` 移植而来。

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::RwLock;

use crate::analysis::tool_extraction::{
    estimate_tokens, extract_tool_results, ToolResultInfo, ToolUseInfo,
};
use crate::analysis::tool_summary_formatter::{format_tokens, get_tool_summary};
use crate::error::error_message_builder::{
    create_detected_error, extract_error_message, find_tool_name_by_tool_use_id,
    CreateDetectedErrorParams,
};
use crate::error::trigger_matcher::{
    extract_tool_use_field, get_content_blocks, matches_ignore_patterns, matches_pattern,
};
use crate::types::config::{
    DetectedError, NotificationTrigger, TriggerMode, TriggerTokenType,
};
use crate::types::domain::MessageType;
use crate::types::messages::{ParsedMessage, ToolCall};
use crate::utils::path_decoder::extract_project_name;

// =============================================================================
// 仓库范围检查
// =============================================================================

/// 线程安全的 projectId -> repositoryId 映射缓存。
static REPOSITORY_ID_CACHE: LazyLock<RwLock<HashMap<String, Option<String>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// 预解析仓库 ID 的目标。
#[derive(Debug, Clone)]
pub struct RepositoryScopeTarget {
    pub project_id: String,
    pub cwd_hint: Option<String>,
}

/// 检查项目是否匹配触发器的仓库范围。
///
/// 若未指定仓库 ID，则触发器适用于所有仓库。
/// 若指定了仓库 ID 但无法从缓存中解析项目的仓库 ID，
/// 则触发器不匹配。
///
/// # 参数
/// * `project_id` - 编码后的项目 ID（例如 "-Users-username-myproject"）
/// * `repository_ids` - 可选的仓库组 ID 列表，用于限定触发器作用范围
///
/// # 返回值
/// 若触发器应应用于此项目则返回 `true`，否则返回 `false`。
pub fn matches_repository_scope(
    project_id: &str,
    repository_ids: Option<&[String]>,
) -> bool {
    // 未指定仓库 ID 时，触发器适用于所有仓库
    let ids = match repository_ids {
        Some(ids) if !ids.is_empty() => ids,
        _ => return true,
    };

    // 从缓存中获取此项目的仓库 ID
    let repo_id = {
        let cache = REPOSITORY_ID_CACHE
            .read()
            .expect("repository_id_cache lock poisoned");
        cache.get(project_id).cloned()
    };

    let resolved_id = match repo_id {
        Some(Some(id)) => id,
        _ => return false,
    };

    // 检查仓库 ID 是否匹配任意已配置的 ID
    ids.iter().any(|id| id == &resolved_id)
}

/// 预解析多个项目的仓库 ID。
///
/// 在检查触发器之前调用此函数以填充缓存。
/// 使用 `GitIdentityResolver` 进行实际解析。
///
/// # 参数
/// * `targets` - 待解析的项目 ID 列表（或带 cwd 提示的目标）
pub fn pre_resolve_repository_ids(_targets: &[RepositoryScopeTarget]) {
    // TODO: 使用 GitIdentityResolver 实现实际的仓库 ID 解析。
    // 当前为空实现 —— 待 Tauri 后端中项目路径解析器可用后，
    // 再实现仓库过滤功能。
}

// =============================================================================
// 工具结果触发器检查
// =============================================================================

/// 检查 tool_result 是否匹配触发器。
///
/// 处理两种模式：
/// - `require_error`：仅当工具结果的 `is_error` 为 true 时匹配
/// - 内容匹配：将工具结果内容与触发器 pattern 进行匹配
///
/// # 参数
/// * `message` - 待检查的已解析消息
/// * `trigger` - 通知触发器配置
/// * `tool_use_map` - tool_use ID 到 ToolUseInfo 的映射
/// * `session_id` - 会话标识符
/// * `project_id` - 项目标识符
/// * `file_path` - 源文件路径
/// * `line_number` - 文件中的行号
///
/// # 返回值
/// 若触发器匹配则返回 `DetectedError`，否则返回 `None`。
pub fn check_tool_result_trigger(
    message: &ParsedMessage,
    trigger: &NotificationTrigger,
    tool_use_map: &HashMap<String, ToolUseInfo>,
    session_id: &str,
    project_id: &str,
    file_path: &str,
    line_number: u64,
) -> Option<DetectedError> {
    let tool_results = extract_tool_results(message, &|msg, id| {
        find_tool_name_by_tool_use_id(msg, id)
    });

    let ignore_patterns: &[String] = trigger.ignore_patterns.as_deref().unwrap_or(&[]);
    let timestamp_ms = parse_timestamp_to_ms(&message.timestamp);

    for result in &tool_results {
        // 若 requireError 为 true，仅匹配 is_error 为 true 的结果
        if trigger.require_error.unwrap_or(false) {
            if !result.is_error {
                continue;
            }

            // 提取错误消息用于忽略模式检查
            let error_message = extract_error_message(result);

            // 检查忽略模式 —— 若任意匹配则跳过此错误
            if matches_ignore_patterns(&error_message, ignore_patterns) {
                continue;
            }

            // 创建已检测的错误
            return Some(create_detected_error(CreateDetectedErrorParams {
                session_id: session_id.to_string(),
                project_id: project_id.to_string(),
                file_path: file_path.to_string(),
                project_name: extract_project_name(project_id, message.cwd.as_deref()),
                line_number,
                source: result
                    .tool_name
                    .clone()
                    .unwrap_or_else(|| "tool_result".to_string()),
                message: error_message,
                timestamp: timestamp_ms,
                cwd: message.cwd.clone(),
                tool_use_id: Some(result.tool_use_id.clone()),
                subagent_id: None,
                trigger_color: trigger.color.clone(),
                trigger_id: Some(trigger.id.clone()),
                trigger_name: Some(trigger.name.clone()),
            }));
        }

        // 非错误 tool_result 触发器（若指定了 toolName）
        if let Some(ref tool_name) = trigger.tool_name {
            let tool_use = tool_use_map.get(&result.tool_use_id);
            if let Some(info) = tool_use {
                if info.name != *tool_name {
                    continue;
                }
            } else {
                continue;
            }

            // 当 matchField 为 .content. 时，匹配内容
            if trigger.match_field.as_deref() == Some("content") {
                if let Some(ref pattern) = trigger.match_pattern {
                    let content = match &result.content {
                        serde_json::Value::String(s) => s.clone(),
                        other => serde_json::to_string(other).unwrap_or_default(),
                    };

                    if !matches_pattern(&content, pattern) {
                        continue;
                    }
                    if matches_ignore_patterns(&content, ignore_patterns) {
                        continue;
                    }

                    let truncated = truncate_content(&content, 200);
                    let error_msg = format!("Tool result matched: {}", truncated);

                    return Some(create_detected_error(CreateDetectedErrorParams {
                        session_id: session_id.to_string(),
                        project_id: project_id.to_string(),
                        file_path: file_path.to_string(),
                        project_name: extract_project_name(project_id, message.cwd.as_deref()),
                        line_number,
                        source: tool_name.clone(),
                        message: error_msg,
                        timestamp: timestamp_ms,
                        cwd: message.cwd.clone(),
                        tool_use_id: Some(result.tool_use_id.clone()),
                        subagent_id: None,
                        trigger_color: trigger.color.clone(),
                        trigger_id: Some(trigger.id.clone()),
                        trigger_name: Some(trigger.name.clone()),
                    }));
                }
            }
        }
    }

    None
}

// =============================================================================
// 工具调用触发器检查
// =============================================================================

/// 检查 tool_use 是否匹配触发器。
///
/// 遍历内容块查找 `tool_use` 类型的块，
/// 然后检查工具名称过滤、模式匹配和忽略模式。
///
/// # 参数
/// * `message` - 待检查的已解析消息（必须为 assistant 类型）
/// * `trigger` - 通知触发器配置
/// * `session_id` - 会话标识符
/// * `project_id` - 项目标识符
/// * `file_path` - 源文件路径
/// * `line_number` - 文件中的行号
///
/// # 返回值
/// 若触发器匹配则返回 `DetectedError`，否则返回 `None`。
pub fn check_tool_use_trigger(
    message: &ParsedMessage,
    trigger: &NotificationTrigger,
    session_id: &str,
    project_id: &str,
    file_path: &str,
    line_number: u64,
) -> Option<DetectedError> {
    if message.message_type != MessageType::Assistant {
        return None;
    }

    let content_blocks = get_content_blocks(message);
    let ignore_patterns: &[String] = trigger.ignore_patterns.as_deref().unwrap_or(&[]);
    let timestamp_ms = parse_timestamp_to_ms(&message.timestamp);

    for block in &content_blocks {
        let block_type = block.get("type").and_then(|v| v.as_str());
        if block_type != Some("tool_use") {
            continue;
        }

        // 从 JSON 块中提取 tool_use 字段
        let tool_use_id = block.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let tool_use_name = block.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let tool_use_input = block.get("input").cloned().unwrap_or(serde_json::Value::Null);

        // 构建 ToolCall 用于字段提取
        let tool_call = ToolCall {
            id: tool_use_id.to_string(),
            name: tool_use_name.to_string(),
            input: tool_use_input,
            is_task: false,
            task_description: None,
            task_subagent_type: None,
        };

        // 若指定了工具名称则进行过滤
        if let Some(ref trigger_tool_name) = trigger.tool_name {
            if tool_use_name != trigger_tool_name {
                continue;
            }
        }

        // 根据 matchField 提取待匹配的字段
        // 若未指定 matchField（如"任意工具"），则匹配整个 input JSON
        let field_value = if trigger.match_field.is_some() {
            extract_tool_use_field(&tool_call, trigger.match_field.as_deref())
        } else {
            // 匹配整个 input JSON
            let input_str = serde_json::to_string(&tool_call.input).unwrap_or_default();
            if input_str.is_empty() || input_str == "null" {
                None
            } else {
                Some(input_str)
            }
        };

        let field_value = match field_value {
            Some(v) => v,
            None => continue,
        };

        // 检查匹配模式
        if let Some(ref pattern) = trigger.match_pattern {
            if !matches_pattern(&field_value, pattern) {
                continue;
            }
        }

        // 检查忽略模式
        if matches_ignore_patterns(&field_value, ignore_patterns) {
            continue;
        }

        // 匹配成功！
        let field_label = trigger
            .match_field
            .as_deref()
            .unwrap_or("tool_use");
        let truncated = truncate_content(&field_value, 200);
        let error_msg = format!("{}: {}", field_label, truncated);

        return Some(create_detected_error(CreateDetectedErrorParams {
            session_id: session_id.to_string(),
            project_id: project_id.to_string(),
            file_path: file_path.to_string(),
            project_name: extract_project_name(project_id, message.cwd.as_deref()),
            line_number,
            source: tool_use_name.to_string(),
            message: error_msg,
            timestamp: timestamp_ms,
            cwd: message.cwd.clone(),
            tool_use_id: Some(tool_use_id.to_string()),
            subagent_id: None,
            trigger_color: trigger.color.clone(),
            trigger_id: Some(trigger.id.clone()),
            trigger_name: Some(trigger.name.clone()),
        }));
    }

    None
}

// =============================================================================
// Token 阈值触发器检查
// =============================================================================

/// 检查单个 tool_use 块是否超过 token 阈值。
///
/// 为每个超过阈值的 tool_use 返回一个 `DetectedError` 数组。

///
/// Token 计算方式（反映上下文窗口影响）：
/// - 工具调用 token：根据 name + JSON.stringify(input) 估算（进入上下文的部分）
/// - 工具结果 token：根据 tool_result.content 估算（Claude 读取的部分）
/// - 总计 = 调用 + 结果
///
/// # 参数
/// * `message` - 待检查的已解析消息（必须为 assistant 类型）
/// * `trigger` - 通知触发器配置
/// * `tool_result_map` - tool_use ID 到 ToolResultInfo 的映射
/// * `session_id` - 会话标识符
/// * `project_id` - 项目标识符
/// * `file_path` - 源文件路径
/// * `line_number` - 文件中的行号
///
/// # 返回值
/// 每个超过阈值的 tool_use 对应的 `DetectedError` 向量。
pub fn check_token_threshold_trigger(
    message: &ParsedMessage,
    trigger: &NotificationTrigger,
    tool_result_map: &HashMap<String, ToolResultInfo>,
    session_id: &str,
    project_id: &str,
    file_path: &str,
    line_number: u64,
) -> Vec<DetectedError> {
    let mut errors = Vec::new();

    // 仅检查 token_threshold 模式
    if trigger.mode != TriggerMode::TokenThreshold {
        return errors;
    }

    let threshold = match trigger.token_threshold {
        Some(t) if t > 0 => t as usize,
        _ => return errors,
    };

    // 仅检查包含 tool_use 块的 assistant 消息
    if message.message_type != MessageType::Assistant {
        return errors;
    }

    let token_type = trigger.token_type.as_ref().unwrap_or(&TriggerTokenType::Total);
    let ignore_patterns: &[String] = trigger.ignore_patterns.as_deref().unwrap_or(&[]);
    let timestamp_ms = parse_timestamp_to_ms(&message.timestamp);

    // 从消息中收集所有 tool_use 块，避免重复
    let mut seen_ids: HashMap<String, bool> = HashMap::new();
    let mut tool_use_blocks: Vec<(String, String, serde_json::Value)> = Vec::new();

    // 从 content 数组中查找 tool_use 块
    if let serde_json::Value::Array(ref blocks) = message.content {
        for block in blocks {
            if block.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                let id = block
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let input = block.get("input").cloned().unwrap_or(serde_json::json!({}));

                if !seen_ids.contains_key(&id) {
                    seen_ids.insert(id.clone(), true);
                    tool_use_blocks.push((id, name, input));
                }
            }
        }
    }

    // 同时检查 toolCalls 数组（若存在）
    for tool_call in &message.tool_calls {
        if !seen_ids.contains_key(&tool_call.id) {
            seen_ids.insert(tool_call.id.clone(), true);
            tool_use_blocks.push((
                tool_call.id.clone(),
                tool_call.name.clone(),
                tool_call.input.clone(),
            ));
        }
    }

    if tool_use_blocks.is_empty() {
        return errors;
    }

    // 逐个检查每个 tool_use 块
    for (tool_use_id, tool_use_name, tool_use_input) in &tool_use_blocks {
        // 若指定了工具名称则进行过滤
        if let Some(ref trigger_tool_name) = trigger.tool_name {
            if tool_use_name != trigger_tool_name {
                continue;
            }
        }

        // 直接从 name + input 计算工具调用的 token 数
        let call_text = format!("{}{}", tool_use_name, tool_use_input);
        let tool_call_tokens = estimate_tokens(&serde_json::Value::String(call_text));

        // 计算工具结果的 token 数（Claude 回读的部分）
        let tool_result_tokens = tool_result_map
            .get(tool_use_id)
            .map(|result| estimate_tokens(&result.content))
            .unwrap_or(0);

        // 根据 tokenType 计算 token 数量
        let token_count = match token_type {
            TriggerTokenType::Input => tool_call_tokens,
            TriggerTokenType::Output => tool_result_tokens,
            TriggerTokenType::Total => tool_call_tokens + tool_result_tokens,
        };

        // 检查阈值
        if token_count <= threshold {
            continue;
        }

        // 构建工具摘要信息
        let tool_summary = get_tool_summary(tool_use_name, tool_use_input);

        // 构建包含工具信息和 token 类型的消息
        let token_type_label = match token_type {
            TriggerTokenType::Total => String::new(),
            other => format!(" {}", serde_json::to_string(other).unwrap_or_default()),
        };
        let token_message = format!(
            "{} - {} : ~{}{} tokens",
            tool_use_name,
            tool_summary,
            format_tokens(token_count),
            token_type_label
        );

        // 检查忽略模式
        if matches_ignore_patterns(&token_message, ignore_patterns) {
            continue;
        }

        errors.push(create_detected_error(CreateDetectedErrorParams {
            session_id: session_id.to_string(),
            project_id: project_id.to_string(),
            file_path: file_path.to_string(),
            project_name: extract_project_name(project_id, message.cwd.as_deref()),
            line_number,
            source: tool_use_name.clone(),
            message: token_message,
            timestamp: timestamp_ms,
            cwd: message.cwd.clone(),
            tool_use_id: Some(tool_use_id.clone()),
            subagent_id: None,
            trigger_color: trigger.color.clone(),
            trigger_id: Some(trigger.id.clone()),
            trigger_name: Some(trigger.name.clone()),
        }));
    }

    errors
}

// =============================================================================
// 辅助函数
// =============================================================================

/// 将内容截断到最大长度用于显示。
fn truncate_content(content: &str, max_len: usize) -> String {
    if content.len() <= max_len {
        return content.to_string();
    }
    // 尝试在字符边界处截断
    let end = content
        .char_indices()
        .take_while(|(i, _)| *i < max_len)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(max_len.min(content.len()));
    format!("{}...", &content[..end])
}

/// 将时间戳字符串解析为自纪元以来的毫秒数。
///
/// 支持 ISO 8601 格式字符串和毫秒时间戳。
fn parse_timestamp_to_ms(timestamp: &str) -> u64 {
    // 首先尝试解析为毫秒时间戳
    if let Ok(ms) = timestamp.parse::<u64>() {
        return ms;
    }

    // 尝试 ISO 8601 解析
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(timestamp) {
        return dt.timestamp_millis() as u64;
    }

    // 回退：返回 0
    0
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::config::{TriggerContentType, TriggerTokenType};
    use crate::types::messages::{ToolCall, ToolResult};
    use serde_json::json;

    // ---------------------------------------------------------------------------
    // 辅助函数
    // ---------------------------------------------------------------------------

    fn make_trigger(
        mode: TriggerMode,
        content_type: TriggerContentType,
    ) -> NotificationTrigger {
        NotificationTrigger {
            id: "trigger-1".to_string(),
            name: "Test Trigger".to_string(),
            enabled: true,
            content_type,
            tool_name: None,
            is_builtin: None,
            ignore_patterns: None,
            mode,
            require_error: None,
            match_field: None,
            match_pattern: None,
            token_threshold: None,
            token_type: None,
            repository_ids: None,
            color: None,
        }
    }

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

    fn make_content_match_trigger(
        tool_name: &str,
        pattern: &str,
    ) -> NotificationTrigger {
        NotificationTrigger {
            id: "content-trigger".to_string(),
            name: "Content Match".to_string(),
            enabled: true,
            content_type: TriggerContentType::ToolResult,
            tool_name: Some(tool_name.to_string()),
            is_builtin: None,
            ignore_patterns: None,
            mode: TriggerMode::ContentMatch,
            require_error: None,
            match_field: Some("content".to_string()),
            match_pattern: Some(pattern.to_string()),
            token_threshold: None,
            token_type: None,
            repository_ids: None,
            color: None,
        }
    }

    fn make_tool_use_trigger(
        tool_name: Option<&str>,
        pattern: Option<&str>,
    ) -> NotificationTrigger {
        NotificationTrigger {
            id: "tool-use-trigger".to_string(),
            name: "Tool Use Trigger".to_string(),
            enabled: true,
            content_type: TriggerContentType::ToolUse,
            tool_name: tool_name.map(String::from),
            is_builtin: None,
            ignore_patterns: None,
            mode: TriggerMode::ContentMatch,
            require_error: None,
            match_field: None,
            match_pattern: pattern.map(String::from),
            token_threshold: None,
            token_type: None,
            repository_ids: None,
            color: None,
        }
    }

    fn make_token_threshold_trigger(threshold: u64) -> NotificationTrigger {
        NotificationTrigger {
            id: "token-trigger".to_string(),
            name: "Token Threshold".to_string(),
            enabled: true,
            content_type: TriggerContentType::ToolResult,
            tool_name: None,
            is_builtin: None,
            ignore_patterns: None,
            mode: TriggerMode::TokenThreshold,
            require_error: None,
            match_field: None,
            match_pattern: None,
            token_threshold: Some(threshold),
            token_type: Some(TriggerTokenType::Total),
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

    // ---------------------------------------------------------------------------
    // matches_repository_scope tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_matches_repository_scope_no_ids() {
        // No repository IDs -> always matches
        assert!(matches_repository_scope("-Users-test-project", None));
        assert!(matches_repository_scope("-Users-test-project", Some(&[])));
    }

    #[test]
    fn test_matches_repository_scope_cached_match() {
        let mut cache = REPOSITORY_ID_CACHE
            .write()
            .expect("repository_id_cache lock poisoned");
        cache.insert(
            "-Users-test-project".to_string(),
            Some("repo-abc-123".to_string()),
        );
        drop(cache);

        let result = matches_repository_scope(
            "-Users-test-project",
            Some(&["repo-abc-123".to_string()]),
        );
        assert!(result);
    }

    #[test]
    fn test_matches_repository_scope_cached_no_match() {
        let mut cache = REPOSITORY_ID_CACHE
            .write()
            .expect("repository_id_cache lock poisoned");
        cache.insert(
            "-Users-test-project".to_string(),
            Some("repo-xyz-789".to_string()),
        );
        drop(cache);

        let result = matches_repository_scope(
            "-Users-test-project",
            Some(&["repo-abc-123".to_string()]),
        );
        assert!(!result);
    }

    #[test]
    fn test_matches_repository_scope_not_cached() {
        // Not in cache -> should return false
        let result = matches_repository_scope(
            "-Users-unknown-project",
            Some(&["repo-abc-123".to_string()]),
        );
        assert!(!result);
    }

    #[test]
    fn test_matches_repository_scope_cached_none() {
        // Cached as None (unresolvable) -> should return false
        let mut cache = REPOSITORY_ID_CACHE
            .write()
            .expect("repository_id_cache lock poisoned");
        cache.insert("-Users-test-project".to_string(), None);
        drop(cache);

        let result = matches_repository_scope(
            "-Users-test-project",
            Some(&["repo-abc-123".to_string()]),
        );
        assert!(!result);
    }

    #[test]
    fn test_pre_resolve_repository_ids_noop() {
        // Just verify it doesn't panic
        let targets = vec![RepositoryScopeTarget {
            project_id: "-Users-test".to_string(),
            cwd_hint: None,
        }];
        pre_resolve_repository_ids(&targets);
    }

    // ---------------------------------------------------------------------------
    // check_tool_result_trigger tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_check_tool_result_trigger_error_status_match() {
        let msg = make_assistant_message(
            json!("test"),
            vec![make_tool_call(
                "tc1",
                "Bash",
                json!({"command": "npm build"}),
            )],
            vec![make_tool_result("tc1", json!("Build failed: error E0425"), true)],
        );

        let trigger = make_error_trigger();
        let tool_use_map = HashMap::new();

        let result = check_tool_result_trigger(
            &msg,
            &trigger,
            &tool_use_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            42,
        );

        assert!(result.is_some());
        let error = result.unwrap();
        assert_eq!(error.source, "Bash");
        assert_eq!(error.message, "Build failed: error E0425");
        assert_eq!(error.trigger_id, Some("error-trigger".to_string()));
        assert_eq!(error.trigger_color, Some("red".to_string()));
    }

    #[test]
    fn test_check_tool_result_trigger_error_status_no_error() {
        let msg = make_assistant_message(
            json!("test"),
            vec![make_tool_call(
                "tc1",
                "Bash",
                json!({"command": "npm build"}),
            )],
            vec![make_tool_result("tc1", json!("Build succeeded"), false)],
        );

        let trigger = make_error_trigger();
        let tool_use_map = HashMap::new();

        let result = check_tool_result_trigger(
            &msg,
            &trigger,
            &tool_use_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            42,
        );

        assert!(result.is_none());
    }

    #[test]
    fn test_check_tool_result_trigger_error_status_ignored() {
        let trigger = NotificationTrigger {
            ignore_patterns: Some(vec!["E0425".to_string()]),
            ..make_error_trigger()
        };

        let msg = make_assistant_message(
            json!("test"),
            vec![make_tool_call(
                "tc1",
                "Bash",
                json!({"command": "npm build"}),
            )],
            vec![make_tool_result("tc1", json!("Build failed: error E0425"), true)],
        );

        let tool_use_map = HashMap::new();

        let result = check_tool_result_trigger(
            &msg,
            &trigger,
            &tool_use_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            42,
        );

        assert!(result.is_none());
    }

    #[test]
    fn test_check_tool_result_trigger_content_match() {
        let trigger = make_content_match_trigger("Bash", "failed");

        let msg = make_assistant_message(
            json!("test"),
            vec![make_tool_call(
                "tc1",
                "Bash",
                json!({"command": "npm build"}),
            )],
            vec![make_tool_result("tc1", json!("Build failed"), false)],
        );

        let mut tool_use_map = HashMap::new();
        tool_use_map.insert(
            "tc1".to_string(),
            crate::analysis::tool_extraction::ToolUseInfo {
                name: "Bash".to_string(),
                input: json!({"command": "npm build"}),
            },
        );

        let result = check_tool_result_trigger(
            &msg,
            &trigger,
            &tool_use_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            42,
        );

        assert!(result.is_some());
        let error = result.unwrap();
        assert_eq!(error.source, "Bash");
        assert!(error.message.contains("Tool result matched"));
    }

    #[test]
    fn test_check_tool_result_trigger_content_match_wrong_tool() {
        let trigger = make_content_match_trigger("Read", "failed");

        let msg = make_assistant_message(
            json!("test"),
            vec![make_tool_call(
                "tc1",
                "Bash",
                json!({"command": "npm build"}),
            )],
            vec![make_tool_result("tc1", json!("Build failed"), false)],
        );

        let mut tool_use_map = HashMap::new();
        tool_use_map.insert(
            "tc1".to_string(),
            crate::analysis::tool_extraction::ToolUseInfo {
                name: "Bash".to_string(),
                input: json!({"command": "npm build"}),
            },
        );

        let result = check_tool_result_trigger(
            &msg,
            &trigger,
            &tool_use_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            42,
        );

        assert!(result.is_none());
    }

    #[test]
    fn test_check_tool_result_trigger_content_match_pattern_miss() {
        let trigger = make_content_match_trigger("Bash", "success");

        let msg = make_assistant_message(
            json!("test"),
            vec![make_tool_call(
                "tc1",
                "Bash",
                json!({"command": "npm build"}),
            )],
            vec![make_tool_result("tc1", json!("Build failed"), false)],
        );

        let mut tool_use_map = HashMap::new();
        tool_use_map.insert(
            "tc1".to_string(),
            crate::analysis::tool_extraction::ToolUseInfo {
                name: "Bash".to_string(),
                input: json!({"command": "npm build"}),
            },
        );

        let result = check_tool_result_trigger(
            &msg,
            &trigger,
            &tool_use_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            42,
        );

        assert!(result.is_none());
    }

    #[test]
    fn test_check_tool_result_trigger_no_results() {
        let msg = make_assistant_message(json!("test"), vec![], vec![]);
        let trigger = make_error_trigger();
        let tool_use_map = HashMap::new();

        let result = check_tool_result_trigger(
            &msg,
            &trigger,
            &tool_use_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            42,
        );

        assert!(result.is_none());
    }

    // ---------------------------------------------------------------------------
    // check_tool_use_trigger tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_check_tool_use_trigger_match() {
        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Bash",
                "input": {"command": "rm -rf /"}
            }
        ]);

        let msg = make_assistant_message(content, vec![], vec![]);
        let trigger = make_tool_use_trigger(Some("Bash"), Some("rm"));

        let result = check_tool_use_trigger(
            &msg,
            &trigger,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert!(result.is_some());
        let error = result.unwrap();
        assert_eq!(error.source, "Bash");
        assert!(error.message.contains("tool_use"));
    }

    #[test]
    fn test_check_tool_use_trigger_no_match_pattern() {
        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Bash",
                "input": {"command": "rm -rf /"}
            }
        ]);

        let msg = make_assistant_message(content, vec![], vec![]);
        let trigger = make_tool_use_trigger(Some("Bash"), Some("git.*push"));

        let result = check_tool_use_trigger(
            &msg,
            &trigger,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert!(result.is_none());
    }

    #[test]
    fn test_check_tool_use_trigger_wrong_tool_name() {
        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Read",
                "input": {"file_path": "/test.rs"}
            }
        ]);

        let msg = make_assistant_message(content, vec![], vec![]);
        let trigger = make_tool_use_trigger(Some("Bash"), Some("rm"));

        let result = check_tool_use_trigger(
            &msg,
            &trigger,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert!(result.is_none());
    }

    #[test]
    fn test_check_tool_use_trigger_ignored() {
        let trigger = NotificationTrigger {
            ignore_patterns: Some(vec!["safe.*command".to_string()]),
            ..make_tool_use_trigger(None, Some("echo"))
        };

        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Bash",
                "input": {"command": "safe command here"}
            }
        ]);

        let msg = make_assistant_message(content, vec![], vec![]);

        let result = check_tool_use_trigger(
            &msg,
            &trigger,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert!(result.is_none());
    }

    #[test]
    fn test_check_tool_use_trigger_non_assistant() {
        let msg = ParsedMessage {
            uuid: "user-1".to_string(),
            parent_uuid: None,
            message_type: MessageType::User,
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
            role: Some("user".to_string()),
            content: json!("hello"),
            usage: None,
            model: None,
            cwd: None,
            git_branch: None,
            agent_id: None,
            is_sidechain: false,
            is_meta: false,
            user_type: None,
            tool_calls: vec![],
            tool_results: vec![],
            source_tool_use_id: None,
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            is_compact_summary: None,
            request_id: None,
        };

        let trigger = make_tool_use_trigger(None, Some("anything"));
        let result = check_tool_use_trigger(
            &msg,
            &trigger,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert!(result.is_none());
    }

    #[test]
    fn test_check_tool_use_trigger_with_match_field() {
        let trigger = NotificationTrigger {
            match_field: Some("command".to_string()),
            ..make_tool_use_trigger(None, Some("rm"))
        };

        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Bash",
                "input": {"command": "rm -rf /tmp/test"}
            }
        ]);

        let msg = make_assistant_message(content, vec![], vec![]);

        let result = check_tool_use_trigger(
            &msg,
            &trigger,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert!(result.is_some());
        let error = result.unwrap();
        assert!(error.message.contains("command"));
    }

    #[test]
    fn test_check_tool_use_trigger_any_tool_no_pattern() {
        // No tool name, no pattern -- should match any tool_use block
        let trigger = make_tool_use_trigger(None, None);

        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Read",
                "input": {"file_path": "/test.rs"}
            }
        ]);

        let msg = make_assistant_message(content, vec![], vec![]);

        let result = check_tool_use_trigger(
            &msg,
            &trigger,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert!(result.is_some());
    }

    // ---------------------------------------------------------------------------
    // check_token_threshold_trigger tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_check_token_threshold_trigger_exceeds() {
        // Create content with enough text to exceed a low threshold
        let large_input = json!({"file_path": "/very/long/path/to/some/deeply/nested/file/that/has/a/very/long/name/and/this/should/generate/lots/of/tokens.rs"});
        let large_result = json!("a".repeat(2000));

        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Read",
                "input": large_input
            }
        ]);

        let msg = make_assistant_message(
            content,
            vec![make_tool_call("tu1", "Read", json!({"file_path": "/test.rs"}))],
            vec![make_tool_result("tu1", large_result, false)],
        );

        let trigger = make_token_threshold_trigger(100);

        let mut tool_result_map = HashMap::new();
        tool_result_map.insert(
            "tu1".to_string(),
            ToolResultInfo {
                content: json!("a".repeat(2000)),
                is_error: false,
            },
        );

        let errors = check_token_threshold_trigger(
            &msg,
            &trigger,
            &tool_result_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].source, "Read");
    }

    #[test]
    fn test_check_token_threshold_trigger_below() {
        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Bash",
                "input": {"command": "ls"}
            }
        ]);

        let msg = make_assistant_message(
            content,
            vec![make_tool_call("tu1", "Bash", json!({"command": "ls"}))],
            vec![make_tool_result("tu1", json!("done"), false)],
        );

        let trigger = make_token_threshold_trigger(100_000);

        let tool_result_map = HashMap::new();

        let errors = check_token_threshold_trigger(
            &msg,
            &trigger,
            &tool_result_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert!(errors.is_empty());
    }

    #[test]
    fn test_check_token_threshold_trigger_wrong_mode() {
        let trigger = make_trigger(TriggerMode::ErrorStatus, TriggerContentType::ToolResult);
        let msg = make_assistant_message(json!("test"), vec![], vec![]);
        let tool_result_map = HashMap::new();

        let errors = check_token_threshold_trigger(
            &msg,
            &trigger,
            &tool_result_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert!(errors.is_empty());
    }

    #[test]
    fn test_check_token_threshold_trigger_non_assistant() {
        let trigger = make_token_threshold_trigger(1);
        let msg = ParsedMessage {
            uuid: "user-1".to_string(),
            parent_uuid: None,
            message_type: MessageType::User,
            timestamp: "2025-01-01T00:00:00.000Z".to_string(),
            role: Some("user".to_string()),
            content: json!("hello"),
            usage: None,
            model: None,
            cwd: None,
            git_branch: None,
            agent_id: None,
            is_sidechain: false,
            is_meta: false,
            user_type: None,
            tool_calls: vec![],
            tool_results: vec![],
            source_tool_use_id: None,
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            is_compact_summary: None,
            request_id: None,
        };

        let errors = check_token_threshold_trigger(
            &msg,
            &trigger,
            &HashMap::new(),
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert!(errors.is_empty());
    }

    #[test]
    fn test_check_token_threshold_trigger_tool_name_filter() {
        let trigger = NotificationTrigger {
            tool_name: Some("Read".to_string()),
            ..make_token_threshold_trigger(100)
        };

        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Bash",
                "input": {"command": "a".repeat(1000)}
            }
        ]);

        let msg = make_assistant_message(
            content,
            vec![make_tool_call("tu1", "Bash", json!({"command": "a".repeat(1000)}))],
            vec![],
        );

        let tool_result_map = HashMap::new();

        let errors = check_token_threshold_trigger(
            &msg,
            &trigger,
            &tool_result_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        // Bash should be filtered out by tool_name=Read
        assert!(errors.is_empty());
    }

    #[test]
    fn test_check_token_threshold_trigger_ignore_pattern() {
        let trigger = NotificationTrigger {
            ignore_patterns: Some(vec!["Read.*test\\.rs".to_string()]),
            ..make_token_threshold_trigger(100)
        };

        let large_content = "a".repeat(2000);
        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Read",
                "input": {"file_path": "/test.rs"}
            }
        ]);

        let msg = make_assistant_message(
            content,
            vec![make_tool_call("tu1", "Read", json!({"file_path": "/test.rs"}))],
            vec![make_tool_result("tu1", json!(large_content), false)],
        );

        let mut tool_result_map = HashMap::new();
        tool_result_map.insert(
            "tu1".to_string(),
            ToolResultInfo {
                content: json!(large_content),
                is_error: false,
            },
        );

        let errors = check_token_threshold_trigger(
            &msg,
            &trigger,
            &tool_result_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        // Should be ignored due to pattern match
        assert!(errors.is_empty());
    }

    #[test]
    fn test_check_token_threshold_trigger_zero_threshold() {
        let trigger = make_token_threshold_trigger(0);
        let msg = make_assistant_message(json!("test"), vec![], vec![]);
        let tool_result_map = HashMap::new();

        let errors = check_token_threshold_trigger(
            &msg,
            &trigger,
            &tool_result_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        // Zero threshold should be treated as "not set"
        assert!(errors.is_empty());
    }

    #[test]
    fn test_check_token_threshold_trigger_multiple_tools() {
        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Read",
                "input": {"file_path": "/test.rs"}
            },
            {
                "type": "tool_use",
                "id": "tu2",
                "name": "Read",
                "input": {"file_path": "/other.rs"}
            }
        ]);

        let large_content = "a".repeat(2000);
        let msg = make_assistant_message(
            content,
            vec![
                make_tool_call("tu1", "Read", json!({"file_path": "/test.rs"})),
                make_tool_call("tu2", "Read", json!({"file_path": "/other.rs"})),
            ],
            vec![
                make_tool_result("tu1", json!(large_content.clone()), false),
                make_tool_result("tu2", json!(large_content), false),
            ],
        );

        let trigger = make_token_threshold_trigger(100);

        let mut tool_result_map = HashMap::new();
        tool_result_map.insert(
            "tu1".to_string(),
            ToolResultInfo {
                content: json!("a".repeat(2000)),
                is_error: false,
            },
        );
        tool_result_map.insert(
            "tu2".to_string(),
            ToolResultInfo {
                content: json!("a".repeat(2000)),
                is_error: false,
            },
        );

        let errors = check_token_threshold_trigger(
            &msg,
            &trigger,
            &tool_result_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn test_check_token_threshold_trigger_no_duplicate_from_content_and_tool_calls() {
        // Same tool_use in both content and toolCalls -- should not duplicate
        let content = json!([
            {
                "type": "tool_use",
                "id": "tu1",
                "name": "Read",
                "input": {"file_path": "/test.rs"}
            }
        ]);

        let large_content = "a".repeat(2000);
        let msg = make_assistant_message(
            content,
            vec![make_tool_call("tu1", "Read", json!({"file_path": "/test.rs"}))],
            vec![make_tool_result("tu1", json!(large_content), false)],
        );

        let trigger = make_token_threshold_trigger(100);

        let mut tool_result_map = HashMap::new();
        tool_result_map.insert(
            "tu1".to_string(),
            ToolResultInfo {
                content: json!("a".repeat(2000)),
                is_error: false,
            },
        );

        let errors = check_token_threshold_trigger(
            &msg,
            &trigger,
            &tool_result_map,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
            10,
        );

        // Should not duplicate -- only one error for tu1
        assert_eq!(errors.len(), 1);
    }

    // ---------------------------------------------------------------------------
    // Helper function tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_truncate_content_short() {
        assert_eq!(truncate_content("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_content_long() {
        let result = truncate_content("a".repeat(100).as_str(), 50);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 53); // 50 chars + "..."
    }

    #[test]
    fn test_parse_timestamp_to_ms_iso() {
        let ms = parse_timestamp_to_ms("2025-01-01T00:00:00.000Z");
        assert!(ms > 0);
        assert_eq!(ms, 1735689600000);
    }

    #[test]
    fn test_parse_timestamp_to_ms_number() {
        let ms = parse_timestamp_to_ms("1700000000000");
        assert_eq!(ms, 1700000000000);
    }

    #[test]
    fn test_parse_timestamp_to_ms_invalid() {
        let ms = parse_timestamp_to_ms("not-a-timestamp");
        assert_eq!(ms, 0);
    }
}
