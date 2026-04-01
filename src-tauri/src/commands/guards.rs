// src-tauri/src/commands/guards.rs
use once_cell::sync::Lazy;
use regex::Regex;

static SESSION_ID_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}$").unwrap());

pub fn validate_project_id(project_id: &str) -> Result<String, String> {
    let trimmed = project_id.trim();
    if trimmed.is_empty() {
        return Err("projectId cannot be empty".into());
    }
    if trimmed.len() > 256 {
        return Err("projectId exceeds max length".into());
    }
    if !crate::utils::path_decoder::is_valid_project_id(trimmed) {
        return Err("projectId contains invalid characters".into());
    }
    Ok(trimmed.to_string())
}

pub fn validate_session_id(session_id: &str) -> Result<String, String> {
    let trimmed = session_id.trim();
    if trimmed.is_empty() {
        return Err("sessionId cannot be empty".into());
    }
    if trimmed.len() > 128 || !SESSION_ID_PATTERN.is_match(trimmed) {
        return Err("sessionId contains invalid characters".into());
    }
    Ok(trimmed.to_string())
}

pub fn validate_subagent_id(subagent_id: &str) -> Result<String, String> {
    validate_session_id(subagent_id) // same pattern
}

pub fn coerce_limit(value: Option<u32>, default: u32, max: u32) -> u32 {
    match value {
        Some(v) if v > 0 => v.min(max),
        _ => default,
    }
}

/// 验证 notification ID。
/// 规则：[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}
pub fn validate_notification_id(id: &str) -> Result<String, String> {
    validate_session_id(id) // 同一模式
}

/// 验证 trigger ID。
/// 规则：[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}
pub fn validate_trigger_id(id: &str) -> Result<String, String> {
    validate_session_id(id) // 同一模式
}

/// 验证搜索查询。
/// 规则：非空 + 长度 <= 512
pub fn validate_search_query(query: &str) -> Result<String, String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Err("query cannot be empty".into());
    }
    if trimmed.len() > 512 {
        return Err("query exceeds max length (512)".into());
    }
    Ok(trimmed.to_string())
}

/// 强制限制搜索最大结果数。默认 50，上限 200。
pub fn coerce_search_max_results(value: Option<u32>) -> u32 {
    coerce_limit(value, 50, 200)
}

/// 强制限制分页大小。默认 20，上限 200。
pub fn coerce_page_limit(value: Option<u32>) -> u32 {
    coerce_limit(value, 20, 200)
}
