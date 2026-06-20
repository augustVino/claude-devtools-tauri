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
    // Bug B4 fix: add input validation (align with HTTP version)
    let context_id = context_id.trim();
    if context_id.is_empty() || context_id.len() > 256 {
        return Err("Invalid context_id".to_string());
    }

    let mut mgr = manager.write().await;

    // Use switch_with_watcher_actions (Batch 1 Task 1)
    let (result, actions) = mgr
        .switch_with_watcher_actions(&context_id)
        .map_err(|e| e.into_tauri_string())?;
    log::info!(
        "Context switched: {} -> {}",
        result.previous_id,
        result.current_id
    );

    // Execute watcher lifecycle based on actions
    if actions.should_stop_old || actions.should_start_new {
        let cm = app
            .state::<Arc<crate::infrastructure::ConfigManager>>()
            .inner()
            .clone();
        let nm = app
            .state::<Arc<RwLock<crate::infrastructure::NotificationManager>>>()
            .inner()
            .clone();
        if actions.should_stop_old {
            if let Some(old_ctx) = mgr.get(&actions.old_context_id) {
                old_ctx.read().await.stop_watcher_tasks().await;
            }
        }
        if actions.should_start_new {
            if let Some(new_ctx) = mgr.get(&actions.new_context_id) {
                let new = new_ctx.read().await;
                new.spawn_watcher_tasks(app.clone(), cm, nm).await;
            }
        }
    }

    // Emit events only when actually switched (drop lock before emit)
    if result.previous_id != result.current_id {
        let ctx_arc = mgr.get(&result.current_id).unwrap();
        let info = ContextInfo::from_context(&*ctx_arc.read().await);
        drop(mgr); // Release write lock before emitting
        events::emit_context_changed(&app, &info);

        // Bridge to SSE
        if let Some(broadcaster) = app.try_state::<crate::http::sse::SSEBroadcaster>() {
            broadcaster
                .inner()
                .send(crate::http::sse::BackendEvent::ContextChanged(info));
        }
    } else {
        drop(mgr);
    }

    Ok(SwitchResponse {
        context_id: result.current_id,
    })
}
