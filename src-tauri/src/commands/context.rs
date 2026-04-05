//! 上下文切换命令 — Tauri IPC 处理函数。

use std::sync::Arc;
use tauri::{command, AppHandle, Manager, State};
use tokio::sync::RwLock;

use crate::events;
use crate::infrastructure::context_manager::{ContextInfo, SwitchResponse};
use crate::infrastructure::ContextManager;

/// 列出所有已注册的上下文。
#[command]
pub async fn context_list(
    manager: State<'_, Arc<RwLock<ContextManager>>>,
) -> Result<Vec<ContextInfo>, String> {
    Ok(manager.read().await.list())
}

/// 获取当前活跃上下文 ID。
#[command]
pub async fn context_active(
    manager: State<'_, Arc<RwLock<ContextManager>>>,
) -> Result<String, String> {
    Ok(manager.read().await.get_active_id().to_string())
}

/// 切换到指定上下文。
#[command]
pub async fn context_switch(
    app: AppHandle,
    manager: State<'_, Arc<RwLock<ContextManager>>>,
    context_id: String,
) -> Result<SwitchResponse, String> {
    let mut mgr = manager.write().await;
    let result = mgr.switch(&context_id)
        .map_err(|e| e.into_tauri_string())?;
    log::info!("Context switched: {} -> {}", result.previous_id, result.current_id);

    // 仅在确实切换了上下文时才 stop/start watcher
    if result.previous_id != result.current_id {
        // Stop old context's watcher tasks
        if let Some(old_ctx) = mgr.get(&result.previous_id) {
            old_ctx.read().await.stop_watcher_tasks().await;
        }

        // Start new context's watcher tasks
        if let Some(new_ctx) = mgr.get(&result.current_id) {
            let new = new_ctx.read().await;
            let config_manager = app.state::<Arc<crate::infrastructure::ConfigManager>>()
                .inner().clone();
            let notification_manager = app.state::<Arc<RwLock<crate::infrastructure::NotificationManager>>>()
                .inner().clone();
            new.spawn_watcher_tasks(app.clone(), config_manager, notification_manager).await;
        }
    }

    // 仅在确实切换了上下文时才发射事件（与 Electron 对齐：no-op 时不发事件）
    if result.previous_id != result.current_id {
        // Emit context:changed event
        let ctx_arc = mgr.get(&result.current_id).unwrap();
        let info = ContextInfo::from_context(&*ctx_arc.read().await);
        drop(mgr);
        events::emit_context_changed(&app, &info);

        // Bridge to SSE
        if let Some(broadcaster) = app.try_state::<crate::http::sse::SSEBroadcaster>() {
            broadcaster.inner().send(crate::http::sse::BackendEvent::ContextChanged(info));
        }
    } else {
        drop(mgr);
    }

    Ok(SwitchResponse { context_id: result.current_id })
}
