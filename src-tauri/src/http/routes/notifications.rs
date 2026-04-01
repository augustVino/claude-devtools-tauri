//! Notifications 路由处理器。
//!
//! 对应 Tauri 命令：notifications.rs 中的通知管理命令。

use axum::{Json, extract::State, http::StatusCode};

use crate::commands::guards;
use crate::http::state::HttpState;
use crate::types::config::{
    GetNotificationsOptions, GetNotificationsResult, NotificationStats,
};

use super::error_json;

/// 获取分页通知列表（按时间降序）。
///
/// GET /api/notifications
pub async fn get_notifications(
    State(state): State<HttpState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<GetNotificationsResult>, (StatusCode, Json<super::ErrorResponse>)> {
    let limit = params
        .get("limit")
        .and_then(|v| v.parse::<u32>().ok());
    let offset = params
        .get("offset")
        .and_then(|v| v.parse::<usize>().ok());

    let mgr = state.notification_manager.read().await;
    let clamped_limit = guards::coerce_page_limit(limit) as usize;
    let clamped_offset = offset.unwrap_or(0);
    let opts = GetNotificationsOptions {
        limit: Some(clamped_limit),
        offset: Some(clamped_offset),
    };
    Ok(Json(mgr.get_notifications(opts).await))
}

/// 标记单条通知为已读。
///
/// POST /api/notifications/{id}/read
pub async fn mark_read(
    State(state): State<HttpState>,
    axum::extract::Path(notification_id): axum::extract::Path<String>,
) -> Result<Json<bool>, (StatusCode, Json<super::ErrorResponse>)> {
    let safe_id = guards::validate_notification_id(&notification_id).map_err(error_json)?;
    let mgr = state.notification_manager.read().await;
    Ok(Json(mgr.mark_read(&safe_id).await))
}

/// 标记所有通知为已读。
///
/// POST /api/notifications/read-all
pub async fn mark_all_read(
    State(state): State<HttpState>,
) -> Result<Json<bool>, (StatusCode, Json<super::ErrorResponse>)> {
    let mgr = state.notification_manager.read().await;
    Ok(Json(mgr.mark_all_read().await))
}

/// 删除单条通知。
///
/// DELETE /api/notifications/{id}
pub async fn delete_notification(
    State(state): State<HttpState>,
    axum::extract::Path(notification_id): axum::extract::Path<String>,
) -> Result<Json<bool>, (StatusCode, Json<super::ErrorResponse>)> {
    let safe_id = guards::validate_notification_id(&notification_id).map_err(error_json)?;
    let mgr = state.notification_manager.read().await;
    Ok(Json(mgr.delete_notification(&safe_id).await))
}

/// 清除所有通知。
///
/// DELETE /api/notifications
pub async fn clear_notifications(
    State(state): State<HttpState>,
) -> Result<Json<bool>, (StatusCode, Json<super::ErrorResponse>)> {
    let mgr = state.notification_manager.read().await;
    Ok(Json(mgr.clear_all().await))
}

/// 获取未读通知数。
///
/// GET /api/notifications/unread-count
pub async fn get_unread_count(
    State(state): State<HttpState>,
) -> Result<Json<usize>, (StatusCode, Json<super::ErrorResponse>)> {
    let mgr = state.notification_manager.read().await;
    let unread_count = mgr.get_unread_count().await;
    Ok(Json(unread_count))
}

/// 获取通知统计信息。
///
/// GET /api/notifications/stats
pub async fn get_stats(
    State(state): State<HttpState>,
) -> Result<Json<NotificationStats>, (StatusCode, Json<super::ErrorResponse>)> {
    let mgr = state.notification_manager.read().await;
    Ok(Json(mgr.get_stats()))
}
