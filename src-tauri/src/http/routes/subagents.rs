//! HTTP 路由处理器：子 Agent 操作。

use std::sync::Arc;
use axum::{Json, extract::State, http::StatusCode};

use crate::commands::guards;
use crate::http::state::HttpState;
use crate::types::chunks::SubagentDetail;

use super::error_json;

/// 获取子 Agent 详情。
///
/// GET /api/projects/{project_id}/sessions/{session_id}/subagents/{subagent_id}
pub async fn get_subagent_detail(
    State(state): State<HttpState>,
    axum::extract::Path((project_id, session_id, subagent_id)): axum::extract::Path<(String, String, String)>,
) -> Result<Json<Option<SubagentDetail>>, (StatusCode, Json<super::ErrorResponse>)> {
    // Validate inputs
    let safe_project_id = match guards::validate_project_id(&project_id) {
        Ok(id) => id,
        Err(e) => {
            log::error!("Invalid projectId: {e}");
            return Err(error_json(&e));
        }
    };
    let safe_session_id = match guards::validate_session_id(&session_id) {
        Ok(id) => id,
        Err(e) => {
            log::error!("Invalid sessionId: {e}");
            return Err(error_json(&e));
        }
    };
    let safe_subagent_id = match guards::validate_subagent_id(&subagent_id) {
        Ok(id) => id,
        Err(e) => {
            log::error!("Invalid subagentId: {e}");
            return Err(error_json(&e));
        }
    };

    let app_state = state.app_state.read().await;

    // Check cache
    if let Some(cached_value) = app_state.cache.get_subagent(
        &safe_project_id, &safe_session_id, &safe_subagent_id
    ).await {
        drop(app_state);
        if let Ok(cached) = serde_json::from_value::<SubagentDetail>(cached_value) {
            return Ok(Json(Some(cached)));
        }
    }

    // Build subagent path
    let projects_dir = crate::utils::path_decoder::get_projects_base_path();
    let base_dir = crate::utils::path_decoder::extract_base_dir(&safe_project_id);
    let subagent_path = projects_dir
        .join(&base_dir)
        .join(&safe_session_id)
        .join("subagents")
        .join(format!("agent-{safe_subagent_id}.jsonl"));

    if !subagent_path.exists() {
        return Ok(Json(None));
    }

    // Parse
    let messages = crate::parsing::jsonl_parser::parse_jsonl_file(&subagent_path).await;
    if messages.is_empty() {
        return Ok(Json(None));
    }

    // Resolve nested subagents
    let fs_provider: Arc<dyn crate::infrastructure::fs_provider::FsProvider> = Arc::new(
        crate::infrastructure::fs_provider::LocalFsProvider::new()
    );
    let resolver = crate::discovery::subagent_resolver::SubagentResolver::new(
        projects_dir.clone(),
        fs_provider,
    );
    let nested = resolver.resolve_subagents(
        &safe_project_id, &safe_subagent_id, None, None
    );

    // Convert resolver::Process → types::chunks::Process via From trait
    let nested_chunks: Vec<crate::types::chunks::Process> =
        nested.into_iter().map(Into::into).collect();

    // Build detail
    let detail = crate::analysis::chunk_builder::ChunkBuilder::build_subagent_detail(
        &safe_subagent_id, &messages, &nested_chunks
    );

    // Cache — need to re-acquire read lock after potential drop
    let app_state = state.app_state.read().await;
    if let Ok(value) = serde_json::to_value(&detail) {
        app_state.cache.set_subagent(
            &safe_project_id, &safe_session_id, &safe_subagent_id, value
        ).await;
    }

    Ok(Json(Some(detail)))
}
