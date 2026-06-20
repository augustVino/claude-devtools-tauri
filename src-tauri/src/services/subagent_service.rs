//! SubagentServiceImpl — 子 Agent 详情构建的具体实现。

use crate::error::AppError;
use crate::infrastructure::ContextManager;
use crate::services::subagent_service_trait::SubagentService;
use crate::types::chunks::SubagentDetail;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct SubagentServiceImpl {
    context_manager: Arc<RwLock<ContextManager>>,
}

impl SubagentServiceImpl {
    pub fn new(context_manager: Arc<RwLock<ContextManager>>) -> Self {
        Self { context_manager }
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
        let active_arc = {
            let mgr = self.context_manager.read().await;
            mgr.get_active()
                .ok_or_else(|| AppError::Internal("No active ServiceContext".into()))?
        };
        let (cache, fs_provider, projects_dir) = {
            let ctx = active_arc.read().await;
            (
                ctx.cache.clone(),
                ctx.fs_provider.clone(),
                ctx.projects_dir.clone(),
            )
        };
        // 释放锁后再做长操作。
        // ⚠️ stale read 语义：跨 IPC 调用之间 active context 可能被切换，
        // 此时 cache/fs_provider/projects_dir 是 stale 引用——这是可接受的
        // （已 clone Arc，FsProvider 即便 dispose 也能安全返回错误）。

        // 1. Check cache
        if let Some(cached_value) = cache
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
        let subagent_path = projects_dir
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

        // 4. Resolve nested subagents
        let resolver = crate::discovery::subagent_resolver::SubagentResolver::new(
            projects_dir.clone(),
            fs_provider.clone(),
        );
        let nested = resolver.resolve_subagents(project_id, subagent_id, None, None);

        // ⚠️ 类型转换：resolver::Process → types::chunks::Process
        let nested_chunks: Vec<crate::types::chunks::Process> =
            nested.into_iter().map(Into::into).collect();

        // 5. Build detail
        let detail = crate::analysis::chunk_builder::ChunkBuilder::build_subagent_detail(
            subagent_id,
            &messages,
            &nested_chunks,
        );

        // 6. Cache result
        if let Ok(value) = serde_json::to_value(&detail) {
            cache
                .set_subagent(project_id, session_id, subagent_id, value)
                .await;
        }

        Ok(Some(detail))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::context_manager::ContextManager;
    use std::path::PathBuf;

    /// 构造空 ContextManager（无任何 context 注册），用于测试无 active context 场景。
    fn make_empty_context_manager() -> Arc<RwLock<ContextManager>> {
        Arc::new(RwLock::new(ContextManager::new()))
    }

    /// 构造含 local context 的 ContextManager，用于 ServiceImpl 测试。
    #[allow(dead_code)]
    async fn make_context_manager(
        projects_dir: PathBuf,
        todos_dir: PathBuf,
    ) -> Arc<RwLock<ContextManager>> {
        use crate::infrastructure::service_context::{
            ContextType, ServiceContext, ServiceContextConfig,
        };
        use crate::infrastructure::LocalFsProvider;
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(ServiceContextConfig {
            id: "local".to_string(),
            context_type: ContextType::Local,
            projects_dir,
            todos_dir,
            fs_provider: Arc::new(LocalFsProvider::new()),
            cache: None,
        }))
        .unwrap();
        Arc::new(RwLock::new(mgr))
    }

    #[tokio::test]
    async fn test_get_subagent_detail_returns_error_when_no_active_context() {
        let svc = SubagentServiceImpl::new(make_empty_context_manager());
        let result = svc.get_subagent_detail("proj", "sess", "sub").await;
        assert!(
            matches!(result, Err(AppError::Internal(msg)) if msg.contains("No active ServiceContext"))
        );
    }
}
