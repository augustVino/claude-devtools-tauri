//! HTTP 路由共享状态。
//!
//! Axum Router 只支持单一 State 类型，因此将所有共享资源合并到一个结构体中。

use std::sync::Arc;
use tokio::sync::RwLock;

use crate::commands::AppState;
use crate::http::sse::SSEBroadcaster;
use crate::infrastructure::{ConfigManager, ContextManager, NotificationManager, SshConnectionManager};
use crate::services::{ProjectService, SearchService, SessionService};

/// Axum 路由使用的共享状态 — 合并所有 HTTP 路由需要的资源。
///
/// Axum Router 只有单一 State 类型参数，不能多次调用 `.with_state()`。
/// 因此将 AppState、SSEBroadcaster、NotificationManager、SessionSearcher 合并到一个结构体。
#[derive(Clone)]
pub struct HttpState {
    pub app_handle: tauri::AppHandle,
    pub app_state: Arc<RwLock<AppState>>,
    pub broadcaster: SSEBroadcaster,
    pub config_manager: Arc<ConfigManager>,
    pub notification_manager: Arc<RwLock<NotificationManager>>,
    pub context_manager: Arc<RwLock<ContextManager>>,
    pub ssh_manager: Arc<RwLock<SshConnectionManager>>,

    // ── Domain Services ──
    pub session_service: Arc<SessionService>,
    pub project_service: Arc<ProjectService>,
    pub search_service: Arc<SearchService>,
}
