//! Subagents 路由处理器。
//!
//! 对应 Tauri 命令：subagents.rs 中的子 Agent 命令。

use axum::{Json, extract::State, http::StatusCode};
use serde::Serialize;

use crate::discovery::SubagentResolver;
use crate::http::state::HttpState;
use crate::parsing::parse_session_file;
use crate::types::domain::SessionMetrics;
use crate::utils::get_projects_base_path;

use super::error_json;

/// 子 Agent 详情（包含完整会话数据）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentDetail {
    pub id: String,
    pub file_path: String,
    pub start_time_ms: u64,
    pub end_time_ms: u64,
    pub duration_ms: u64,
    pub is_parallel: bool,
    pub is_ongoing: bool,
    pub metrics: SessionMetrics,
    pub messages: Vec<crate::types::messages::ParsedMessage>,
    pub task_id: Option<String>,
}

/// 路径参数：project_id + session_id + subagent_id。
#[derive(serde::Deserialize)]
pub struct SubagentPath {
    pub project_id: String,
    pub session_id: String,
    pub subagent_id: String,
}

/// 获取子 Agent 详细信息。
///
/// GET /api/projects/{project_id}/sessions/{session_id}/subagents/{subagent_id}
pub async fn get_subagent_detail(
    State(_state): State<HttpState>,
    axum::extract::Path(path): axum::extract::Path<SubagentPath>,
) -> Result<Json<Option<SubagentDetail>>, (StatusCode, Json<super::ErrorResponse>)> {
    let SubagentPath {
        project_id,
        session_id,
        subagent_id,
    } = path;

    let projects_dir = get_projects_base_path();

    // 构建子 Agent 文件路径（文件命名为 "agent-{id}.jsonl"）
    let base_dir = crate::utils::path_decoder::extract_base_dir(&project_id);
    let subagent_path = projects_dir
        .join(base_dir)
        .join(&session_id)
        .join("subagents")
        .join(format!("agent-{}.jsonl", subagent_id));

    if !subagent_path.exists() {
        return Ok(Json(None));
    }

    // 解析子 Agent 文件
    let parsed = parse_session_file(&subagent_path).await;

    // 解析子 Agent 以获取时间/并行信息
    let resolver = SubagentResolver::new(projects_dir);
    let processes = resolver.resolve_subagents(&project_id, &session_id);

    // 查找匹配的进程
    let process = processes.iter().find(|p| p.id == subagent_id);

    let (start_time_ms, end_time_ms, duration_ms, is_parallel, is_ongoing, task_id) = process
        .map(|p| {
            (
                p.start_time_ms,
                p.end_time_ms,
                p.duration_ms,
                p.is_parallel,
                p.is_ongoing,
                p.task_id.clone(),
            )
        })
        .unwrap_or((0, 0, 0, false, false, None));

    Ok(Json(Some(SubagentDetail {
        id: subagent_id,
        file_path: subagent_path.to_string_lossy().to_string(),
        start_time_ms,
        end_time_ms,
        duration_ms,
        is_parallel,
        is_ongoing,
        metrics: parsed.metrics,
        messages: parsed.messages,
        task_id,
    })))
}
