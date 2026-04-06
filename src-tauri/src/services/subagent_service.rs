//! SubagentServiceImpl — 子 Agent 详情构建的具体实现。

use std::path::PathBuf;
use std::sync::Arc;
use async_trait::async_trait;
use crate::error::AppError;
use crate::infrastructure::{DataCache, fs_provider::FsProvider};
use crate::services::subagent_service_trait::SubagentService;
use crate::types::chunks::SubagentDetail;

pub struct SubagentServiceImpl {
    // DataCache 已有 #[derive(Clone)]（内部基于 Arc<Cache> + Arc<AtomicBool>）
    // clone() 仅增加引用计数，性能开销可忽略
    cache: DataCache,
    // 共享 FsProvider 单例（与 SessionServiceImpl、ProjectServiceImpl 等使用同一实例）
    fs_provider: Arc<dyn FsProvider>,
    // projects 基础路径（在 lib.rs run() 中一次性获取，避免每次调用重复计算）
    projects_dir: PathBuf,
}

impl SubagentServiceImpl {
    /// 遵循 SessionServiceImpl 的 DI 模式：所有外部依赖通过构造函数注入。
    ///
    /// `fs_provider` 和 `projects_dir` 来自 lib.rs 中已创建的共享实例，
    /// 不再像原 handler 那样内联 new LocalFsProvider（code smell）。
    pub fn new(
        cache: DataCache,
        fs_provider: Arc<dyn FsProvider>,
        projects_dir: PathBuf,
    ) -> Self {
        Self { cache, fs_provider, projects_dir }
    }
}

#[async_trait]
impl SubagentService for SubagentServiceImpl {
    async fn get_subagent_detail(
        &self,
        project_id: &str,
        session_id: &str,
        subagent_id: &str,
    ) -> Result<Option<SubagentDetail>, AppError> {
        // 1. Check cache
        if let Some(cached_value) = self.cache
            .get_subagent(project_id, session_id, subagent_id)
            .await
        {
            if let Ok(cached) = serde_json::from_value::<SubagentDetail>(cached_value) {
                return Ok(Some(cached));
            }
            // cache corruption — fall through to rebuild
        }

        // 2. Build file path
        let base_dir = crate::utils::path_decoder::extract_base_dir(project_id);
        let subagent_path = self.projects_dir
            .join(&base_dir)
            .join(session_id)
            .join("subagents")
            .join(format!("agent-{subagent_id}.jsonl"));

        if !subagent_path.exists() {
            return Ok(None);
        }

        // 3. Parse JSONL
        let messages = crate::parsing::jsonl_parser::parse_jsonl_file(&subagent_path).await;
        if messages.is_empty() {
            return Ok(None);
        }

        // 4. Resolve nested subagents（复用注入的 fs_provider，不再内联创建）
        let resolver = crate::discovery::subagent_resolver::SubagentResolver::new(
            self.projects_dir.clone(),
            self.fs_provider.clone(),
        );
        let nested = resolver.resolve_subagents(
            project_id, subagent_id, None, None
        );

        // ⚠️ 类型转换：resolver::Process → types::chunks::Process
        //
        // 存在两个同名但字段不同的 Process 类型：
        //   - discovery::subagent_resolver::Process（task_id: Option<String>, is_ongoing: bool）
        //   - types::chunks::Process（parent_task_id: Option<String>, is_ongoing: Option<bool>）
        //
        // chunks.rs:178-197 已实现 From<resolver::Process> for Process，
        // 负责字段映射（task_id→parent_task_id, bool→Some(bool)）。
        // build_subagent_detail 要求 &[_types::chunks::Process]，所以必须在此处转换。
        let nested_chunks: Vec<crate::types::chunks::Process> =
            nested.into_iter().map(Into::into).collect();

        // 5. Build detail
        let detail = crate::analysis::chunk_builder::ChunkBuilder::build_subagent_detail(
            subagent_id, &messages, &nested_chunks
        );

        // 6. Cache result
        if let Ok(value) = serde_json::to_value(&detail) {
            self.cache.set_subagent(
                project_id, session_id, subagent_id, value
            ).await;
        }

        Ok(Some(detail))
    }
}
