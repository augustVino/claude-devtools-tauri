//! Config 路由处理器。
//!
//! 对应 Tauri 命令：config.rs 中的配置管理命令。

use axum::{Json, extract::State, http::StatusCode};
use serde::Serialize;
use serde::Deserialize;

use crate::commands::guards;
use crate::http::state::HttpState;
use crate::types::config::{
    AppConfig, NotificationTrigger,
};

use super::error_json;

/// httpClient.ts 的 config API 期望 `{success, data}` 包装格式。
#[derive(Serialize)]
pub(crate) struct ConfigResponse {
    success: bool,
    data: AppConfig,
}

/// 获取当前完整的应用配置。
///
/// GET /api/config
pub async fn get_config(
    State(state): State<HttpState>,
) -> Result<Json<ConfigResponse>, (StatusCode, Json<super::ErrorResponse>)> {
    let app_state = state.app_state.read().await;
    Ok(Json(ConfigResponse {
        success: true,
        data: app_state.config_manager.get_config().await,
    }))
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
) -> Result<Json<ConfigResponse>, (StatusCode, Json<super::ErrorResponse>)> {
    let result = state.config_svc.update_config(&body.section, body.data).await
        .map_err(|e| error_json(e.to_string()))?;
    Ok(Json(ConfigResponse { success: true, data: result }))
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
        .await
        .map(Json)
        .map_err(|e| error_json(e.to_string()))
}

/// 移除通知忽略正则表达式。
///
/// DELETE /api/config/ignore-regex
pub async fn remove_ignore_regex(
    State(state): State<HttpState>,
    Json(body): Json<PatternRequest>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let app_state = state.app_state.read().await;
    app_state.config_manager.remove_ignore_regex(body.pattern).await
        .map(Json)
        .map_err(|e| error_json(e.to_string()))
}

// =============================================================================
// 仓库忽略
// =============================================================================

/// 请求体：仓库 ID。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
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
    app_state.config_manager.add_ignore_repository(body.repository_id).await
        .map(Json)
        .map_err(|e| error_json(e.to_string()))
}

/// 从忽略列表中移除指定仓库。
///
/// DELETE /api/config/ignore-repository
pub async fn remove_ignore_repository(
    State(state): State<HttpState>,
    Json(body): Json<RepositoryIdRequest>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let app_state = state.app_state.read().await;
    app_state
        .config_manager
        .remove_ignore_repository(body.repository_id).await
        .map(Json)
        .map_err(|e| error_json(e.to_string()))
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
    state.config_svc.snooze(body.minutes).await
        .map(Json)
        .map_err(|e| error_json(e.to_string()))
}

/// 清除通知暂停设置，恢复通知推送。
///
/// POST /api/config/clear-snooze
pub async fn clear_snooze(
    State(state): State<HttpState>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    state.config_svc.clear_snooze().await
        .map(Json)
        .map_err(|e| error_json(e.to_string()))
}

// =============================================================================
// 通知触发器
// =============================================================================

/// 触发器列表响应（包装格式）。
#[derive(Serialize)]
pub(crate) struct TriggersResponse {
    success: bool,
    data: Vec<NotificationTrigger>,
}

/// 获取所有通知触发器列表。
///
/// GET /api/config/triggers
pub async fn get_triggers(
    State(state): State<HttpState>,
) -> Result<Json<TriggersResponse>, (StatusCode, Json<super::ErrorResponse>)> {
    let app_state = state.app_state.read().await;
    Ok(Json(TriggersResponse {
        success: true,
        data: app_state.config_manager.get_triggers().await,
    }))
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
        .await
        .map(Json)
        .map_err(|e| error_json(e.to_string()))
}

/// 更新指定通知触发器的配置。
///
/// PUT /api/config/triggers/{trigger_id}
pub async fn update_trigger(
    State(state): State<HttpState>,
    axum::extract::Path(trigger_id): axum::extract::Path<String>,
    Json(updates): Json<serde_json::Value>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let safe_trigger_id = guards::validate_trigger_id(&trigger_id)
        .map_err(|e| error_json(e.to_string()))?;

    let app_state = state.app_state.read().await;
    app_state
        .config_manager
        .update_trigger(&safe_trigger_id, updates)
        .await
        .map(Json)
        .map_err(|e| error_json(e.to_string()))
}

/// 删除指定通知触发器。
///
/// DELETE /api/config/triggers/{trigger_id}
pub async fn remove_trigger(
    State(state): State<HttpState>,
    axum::extract::Path(trigger_id): axum::extract::Path<String>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let safe_trigger_id = guards::validate_trigger_id(&trigger_id)
        .map_err(|e| error_json(e.to_string()))?;

    let app_state = state.app_state.read().await;
    app_state
        .config_manager
        .remove_trigger(&safe_trigger_id)
        .await
        .map(Json)
        .map_err(|e| error_json(e.to_string()))
}

/// 触发器测试结果响应（包装格式）。
#[derive(Serialize)]
pub(crate) struct TriggerTestResponse {
    success: bool,
    data: crate::types::config::TriggerTestResult,
}

/// 测试通知触发器。
///
/// POST /api/config/triggers/{trigger_id}/test
// 【审查修正】参数名从 _state 改为 state（原计划有下划线前缀导致编译失败）
pub async fn test_trigger(
    state: State<HttpState>,
    _trigger_id: axum::extract::Path<String>,
    Json(trigger): Json<NotificationTrigger>,
) -> Result<
    Json<TriggerTestResponse>,
    (StatusCode, Json<super::ErrorResponse>),
> {
    let _safe_trigger_id = guards::validate_trigger_id(_trigger_id.0.as_str())
        .map_err(|e| error_json(e.to_string()))?;

    let result = state.config_svc.test_trigger(&trigger).await
        .map_err(|e| error_json(e.to_string()))?;
    Ok(Json(TriggerTestResponse { success: true, data: result }))
}

// =============================================================================
// 会话置顶/隐藏
// =============================================================================

/// 请求体：会话标识。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
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
    let _safe_project_id = guards::validate_project_id(&body.project_id)
        .map_err(|e| error_json(e.to_string()))?;
    let _safe_session_id = guards::validate_session_id(&body.session_id)
        .map_err(|e| error_json(e.to_string()))?;

    let app_state = state.app_state.read().await;
    app_state
        .config_manager
        .pin_session(body.project_id, body.session_id).await
        .map(Json)
        .map_err(|e| error_json(e.to_string()))
}

/// 取消置顶指定会话。
///
/// POST /api/config/unpin-session
pub async fn unpin_session(
    State(state): State<HttpState>,
    Json(body): Json<SessionIdentRequest>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let _safe_project_id = guards::validate_project_id(&body.project_id)
        .map_err(|e| error_json(e.to_string()))?;
    let _safe_session_id = guards::validate_session_id(&body.session_id)
        .map_err(|e| error_json(e.to_string()))?;

    let app_state = state.app_state.read().await;
    app_state
        .config_manager
        .unpin_session(body.project_id, body.session_id).await
        .map(Json)
        .map_err(|e| error_json(e.to_string()))
}

/// 隐藏指定会话。
///
/// POST /api/config/hide-session
pub async fn hide_session(
    State(state): State<HttpState>,
    Json(body): Json<SessionIdentRequest>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let _safe_project_id = guards::validate_project_id(&body.project_id)
        .map_err(|e| error_json(e.to_string()))?;
    let _safe_session_id = guards::validate_session_id(&body.session_id)
        .map_err(|e| error_json(e.to_string()))?;

    let app_state = state.app_state.read().await;
    app_state
        .config_manager
        .hide_session(body.project_id, body.session_id).await
        .map(Json)
        .map_err(|e| error_json(e.to_string()))
}

/// 取消隐藏指定会话。
///
/// POST /api/config/unhide-session
pub async fn unhide_session(
    State(state): State<HttpState>,
    Json(body): Json<SessionIdentRequest>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let _safe_project_id = guards::validate_project_id(&body.project_id)
        .map_err(|e| error_json(e.to_string()))?;
    let _safe_session_id = guards::validate_session_id(&body.session_id)
        .map_err(|e| error_json(e.to_string()))?;

    let app_state = state.app_state.read().await;
    app_state
        .config_manager
        .unhide_session(body.project_id, body.session_id).await
        .map(Json)
        .map_err(|e| error_json(e.to_string()))
}

// =============================================================================
// 批量隐藏/取消隐藏
// =============================================================================

/// 请求体：批量会话标识。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
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
    let _safe_project_id = guards::validate_project_id(&body.project_id)
        .map_err(|e| error_json(e.to_string()))?;

    let app_state = state.app_state.read().await;
    app_state
        .config_manager
        .hide_sessions(body.project_id, body.session_ids).await
        .map(Json)
        .map_err(|e| error_json(e.to_string()))
}

/// 批量取消隐藏指定会话。
///
/// POST /api/config/unhide-sessions
pub async fn unhide_sessions(
    State(state): State<HttpState>,
    Json(body): Json<BatchSessionIdentRequest>,
) -> Result<Json<AppConfig>, (StatusCode, Json<super::ErrorResponse>)> {
    let _safe_project_id = guards::validate_project_id(&body.project_id)
        .map_err(|e| error_json(e.to_string()))?;

    let app_state = state.app_state.read().await;
    app_state
        .config_manager
        .unhide_sessions(body.project_id, body.session_ids).await
        .map(Json)
        .map_err(|e| error_json(e.to_string()))
}

// =============================================================================
// No-op（需要原生 UI 交互的路由）
// =============================================================================

/// No-op handler for routes that require native UI interaction (select-folders, open-in-editor).
/// Returns error response aligned with Electron: "Not available in browser mode".
pub async fn no_op() -> (StatusCode, Json<super::ErrorResponse>) {
    super::not_available_response()
}
