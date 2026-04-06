//! IPC Handlers for Subagent Operations.
//!
//! All business logic delegated to SubagentService. Thin adapter: validation + error wrapping.

use std::sync::Arc;
use tauri::{command, State};

use crate::commands::guards;
use crate::error::AppError;
use crate::services::SubagentService;
use crate::types::chunks::SubagentDetail;

#[command]
pub async fn get_subagent_detail(
    subagent_svc: State<'_, Arc<dyn SubagentService>>,
    project_id: String,
    session_id: String,
    subagent_id: String,
) -> Result<Option<SubagentDetail>, String> {
    // Validate inputs (IPC-style error handling)
    let safe_project_id = guards::validate_project_id(&project_id)
        .map_err(|e| { log::error!("Invalid projectId: {e}"); e })?;
    let safe_session_id = guards::validate_session_id(&session_id)
        .map_err(|e| { log::error!("Invalid sessionId: {e}"); e })?;
    let safe_subagent_id = guards::validate_subagent_id(&subagent_id)
        .map_err(|e| { log::error!("Invalid subagentId: {e}"); e })?;

    subagent_svc.get_subagent_detail(&safe_project_id, &safe_session_id, &safe_subagent_id)
        .await
        .map_err(|e: AppError| e.into_tauri_string())
}
