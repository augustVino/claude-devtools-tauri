//! Config 路由处理器。
//!
//! 对应 Tauri 命令：config.rs 中的配置管理命令。

use axum::{Json, extract::State, http::StatusCode};
use serde::Deserialize;

use crate::http::state::HttpState;
use crate::types::config::{
    AppConfig, NotificationTrigger,
};

use super::error_json;

/// 获取当前完整的应用配置。
///
/// GET /api/config
pub async fn get_config(
    State(state): State<HttpState>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let app_state = state.app_state.read().await;
    Ok(Json(app_state.config_manager.get_config()))
}

/// 请求体：更新配置分区。
#[derive(Deserialize)]
pub struct UpdateConfigRequest {
    pub section: String,
    pub data: serde_json::Value,
}

/// 更新配置的指定分区。
///
/// POST /api/config/update
pub async fn update_config(
    State(state): State<HttpState>,
    Json(body): Json<UpdateConfigRequest>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let app_state = state.app_state.read().await;
    app_state
        .config_manager
        .update_config(&body.section, body.data)
        .map(Json)
        .map_err(|e| error_json(e))
}

// =============================================================================
// 通知忽略正则
// =============================================================================

/// 请求体：添加忽略正则。
#[derive(Deserialize)]
pub struct PatternRequest {
    pub pattern: String,
}

/// 添加通知忽略正则表达式。
///
/// POST /api/config/ignore-regex
pub async fn add_ignore_regex(
    State(state): State<HttpState>,
    Json(body): Json<PatternRequest>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let app_state = state.app_state.read().await;
    app_state
        .config_manager
        .add_ignore_regex(body.pattern)
        .map(Json)
        .map_err(|e| error_json(e))
}

/// 移除通知忽略正则表达式。
///
/// DELETE /api/config/ignore-regex
pub async fn remove_ignore_regex(
    State(state): State<HttpState>,
    Json(body): Json<PatternRequest>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let app_state = state.app_state.read().await;
    Ok(Json(app_state.config_manager.remove_ignore_regex(body.pattern)))
}

// =============================================================================
// 仓库忽略
// =============================================================================

/// 请求体：仓库 ID。
#[derive(Deserialize)]
pub struct RepositoryIdRequest {
    pub repository_id: String,
}

/// 添加仓库到忽略列表。
///
/// POST /api/config/ignore-repository
pub async fn add_ignore_repository(
    State(state): State<HttpState>,
    Json(body): Json<RepositoryIdRequest>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let app_state = state.app_state.read().await;
    Ok(Json(
        app_state.config_manager.add_ignore_repository(body.repository_id),
    ))
}

/// 从忽略列表中移除指定仓库。
///
/// DELETE /api/config/ignore-repository
pub async fn remove_ignore_repository(
    State(state): State<HttpState>,
    Json(body): Json<RepositoryIdRequest>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let app_state = state.app_state.read().await;
    Ok(Json(
        app_state
            .config_manager
            .remove_ignore_repository(body.repository_id),
    ))
}

// =============================================================================
// 通知暂停
// =============================================================================

/// 请求体：暂停分钟数。
#[derive(Deserialize)]
pub struct SnoozeRequest {
    pub minutes: i32,
}

/// 暂停通知推送指定分钟数。
///
/// POST /api/config/snooze
pub async fn snooze(
    State(state): State<HttpState>,
    Json(body): Json<SnoozeRequest>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let app_state = state.app_state.read().await;
    if body.minutes == -1 {
        Ok(Json(app_state.config_manager.snooze_until_tomorrow()))
    } else if body.minutes <= 0 {
        Err(error_json("Minutes must be a positive number"))
    } else if body.minutes > 24 * 60 {
        Err(error_json("Minutes must be 1440 or less (24 hours)"))
    } else {
        Ok(Json(app_state.config_manager.snooze(body.minutes as u32)))
    }
}

/// 清除通知暂停设置，恢复通知推送。
///
/// POST /api/config/clear-snooze
pub async fn clear_snooze(
    State(state): State<HttpState>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let app_state = state.app_state.read().await;
    Ok(Json(app_state.config_manager.clear_snooze()))
}

// =============================================================================
// 通知触发器
// =============================================================================

/// 获取所有通知触发器列表。
///
/// GET /api/config/triggers
pub async fn get_triggers(
    State(state): State<HttpState>,
) -> Result<Json<Vec<NotificationTrigger>>, (StatusCode, Json<super::ErrorResponse>)> {
    let app_state = state.app_state.read().await;
    Ok(Json(app_state.config_manager.get_triggers()))
}

/// 添加自定义通知触发器。
///
/// POST /api/config/triggers
pub async fn add_trigger(
    State(state): State<HttpState>,
    Json(trigger): Json<NotificationTrigger>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let app_state = state.app_state.read().await;
    app_state
        .config_manager
        .add_trigger(trigger)
        .map(Json)
        .map_err(|e| error_json(e))
}

/// 更新指定通知触发器的配置。
///
/// PUT /api/config/triggers/{trigger_id}
pub async fn update_trigger(
    State(state): State<HttpState>,
    axum::extract::Path(trigger_id): axum::extract::Path<String>,
    Json(updates): Json<serde_json::Value>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let app_state = state.app_state.read().await;
    app_state
        .config_manager
        .update_trigger(&trigger_id, updates)
        .map(Json)
        .map_err(|e| error_json(e))
}

/// 删除指定通知触发器。
///
/// DELETE /api/config/triggers/{trigger_id}
pub async fn remove_trigger(
    State(state): State<HttpState>,
    axum::extract::Path(trigger_id): axum::extract::Path<String>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let app_state = state.app_state.read().await;
    app_state
        .config_manager
        .remove_trigger(&trigger_id)
        .map(Json)
        .map_err(|e| error_json(e))
}

/// 测试通知触发器。
///
/// POST /api/config/triggers/{trigger_id}/test
pub async fn test_trigger(
    State(_state): State<HttpState>,
    axum::extract::Path(_trigger_id): axum::extract::Path<String>,
    Json(trigger): Json<NotificationTrigger>,
) -> Result<
    Json<crate::types::config::TriggerTestResult>,
    (StatusCode, Json<super::ErrorResponse>),
> {
    use crate::discovery::project_scanner::ProjectScanner;
    use crate::error::error_trigger_tester;

    let scanner = ProjectScanner::new();
    Ok(Json(error_trigger_tester::test_trigger(&trigger, &scanner, None).await))
}

// =============================================================================
// 会话置顶/隐藏
// =============================================================================

/// 请求体：会话标识。
#[derive(Deserialize)]
pub struct SessionIdentRequest {
    pub project_id: String,
    pub session_id: String,
}

/// 置顶指定会话。
///
/// POST /api/config/pin-session
pub async fn pin_session(
    State(state): State<HttpState>,
    Json(body): Json<SessionIdentRequest>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let app_state = state.app_state.read().await;
    Ok(Json(
        app_state
            .config_manager
            .pin_session(body.project_id, body.session_id),
    ))
}

/// 取消置顶指定会话。
///
/// POST /api/config/unpin-session
pub async fn unpin_session(
    State(state): State<HttpState>,
    Json(body): Json<SessionIdentRequest>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let app_state = state.app_state.read().await;
    Ok(Json(
        app_state
            .config_manager
            .unpin_session(body.project_id, body.session_id),
    ))
}

/// 隐藏指定会话。
///
/// POST /api/config/hide-session
pub async fn hide_session(
    State(state): State<HttpState>,
    Json(body): Json<SessionIdentRequest>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let app_state = state.app_state.read().await;
    Ok(Json(
        app_state
            .config_manager
            .hide_session(body.project_id, body.session_id),
    ))
}

/// 取消隐藏指定会话。
///
/// POST /api/config/unhide-session
pub async fn unhide_session(
    State(state): State<HttpState>,
    Json(body): Json<SessionIdentRequest>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let app_state = state.app_state.read().await;
    Ok(Json(
        app_state
            .config_manager
            .unhide_session(body.project_id, body.session_id),
    ))
}

// =============================================================================
// 批量隐藏/取消隐藏
// =============================================================================

/// 请求体：批量会话标识。
#[derive(Deserialize)]
pub struct BatchSessionIdentRequest {
    pub project_id: String,
    pub session_ids: Vec<String>,
}

/// 批量隐藏指定会话。
///
/// POST /api/config/hide-sessions
pub async fn hide_sessions(
    State(state): State<HttpState>,
    Json(body): Json<BatchSessionIdentRequest>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let app_state = state.app_state.read().await;
    Ok(Json(
        app_state
            .config_manager
            .hide_sessions(body.project_id, body.session_ids),
    ))
}

/// 批量取消隐藏指定会话。
///
/// POST /api/config/unhide-sessions
pub async fn unhide_sessions(
    State(state): State<HttpState>,
    Json(body): Json<BatchSessionIdentRequest>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let app_state = state.app_state.read().await;
    Ok(Json(
        app_state
            .config_manager
            .unhide_sessions(body.project_id, body.session_ids),
    ))
}

// =============================================================================
// No-op（需要原生 UI 交互的路由）
// =============================================================================

/// No-op handler for routes that require native UI interaction (select-folders, open-in-editor).
pub async fn no_op() -> (StatusCode, Json<super::SuccessResponse>) {
    super::success_json()
}
