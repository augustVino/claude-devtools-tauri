//! IPC Handlers for Subagent Operations.

use std::sync::Arc;
use tokio::sync::RwLock;
use tauri::{command, State};

use crate::commands::guards;
use crate::commands::AppState;
use crate::types::chunks::SubagentDetail;

#[command]
pub async fn get_subagent_detail(
    state: State<'_, Arc<RwLock<AppState>>>,
    project_id: String,
    session_id: String,
    subagent_id: String,
) -> Result<Option<SubagentDetail>, String> {
    // Validate inputs
    let safe_project_id = guards::validate_project_id(&project_id)
        .map_err(|e| { log::error!("Invalid projectId: {e}"); e })?;
    let safe_session_id = guards::validate_session_id(&session_id)
        .map_err(|e| { log::error!("Invalid sessionId: {e}"); e })?;
    let safe_subagent_id = guards::validate_subagent_id(&subagent_id)
        .map_err(|e| { log::error!("Invalid subagentId: {e}"); e })?;

    let app_state = state.read().await;

    let projects_dir = crate::utils::path_decoder::get_projects_base_path();

    // Check cache — DataCache stores serde_json::Value, deserialize back
    if let Some(cached_value) = app_state.cache.get_subagent(
        &safe_project_id, &safe_session_id, &safe_subagent_id
    ).await {
        drop(app_state);
        if let Ok(cached) = serde_json::from_value::<SubagentDetail>(cached_value) {
            return Ok(Some(cached));
        }
        // cache corruption — fall through to rebuild
    }

    // Build subagent path
    let base_dir = crate::utils::path_decoder::extract_base_dir(&safe_project_id);
    let subagent_path = projects_dir
        .join(&base_dir)
        .join(&safe_session_id)
        .join("subagents")
        .join(format!("agent-{safe_subagent_id}.jsonl"));

    if !subagent_path.exists() {
        return Ok(None);
    }

    // Parse
    let messages = crate::parsing::jsonl_parser::parse_jsonl_file(&subagent_path).await;
    if messages.is_empty() {
        return Ok(None);
    }

    // Resolve nested subagents using SubagentResolver
    let fs_provider: Arc<dyn crate::infrastructure::fs_provider::FsProvider> = Arc::new(
        crate::infrastructure::fs_provider::LocalFsProvider::new()
    );
    let resolver = crate::discovery::subagent_resolver::SubagentResolver::new(
        projects_dir.clone(),
        fs_provider,
    );
    // Pass subagent_id as session_id to resolve nested subagents under the subagent's own directory
    let nested = resolver.resolve_subagents(
        &safe_project_id, &safe_subagent_id, None, None
    );

    // Convert resolver::Process → types::chunks::Process via From trait
    let nested_chunks: Vec<crate::types::chunks::Process> =
        nested.into_iter().map(Into::into).collect();

    // Build detail — ChunkBuilder is a unit struct
    let detail = crate::analysis::chunk_builder::ChunkBuilder::build_subagent_detail(
        &safe_subagent_id, &messages, &nested_chunks
    );

    // Cache — serialize to serde_json::Value for DataCache
    // Re-acquire read lock for cache write
    let app_state = state.read().await;
    if let Ok(value) = serde_json::to_value(&detail) {
        app_state.cache.set_subagent(
            &safe_project_id, &safe_session_id, &safe_subagent_id, value
        ).await;
    }

    Ok(Some(detail))
}
