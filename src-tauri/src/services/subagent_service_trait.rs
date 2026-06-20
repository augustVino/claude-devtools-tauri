//! Subagent Service — 子 Agent 详情构建。
//!
//! 封装 get_subagent_detail 的完整流水线：
//! 校验 → 缓存检查 → 路径构建 → JSONL 解析 → SubagentResolver → ChunkBuilder → 缓存写入

use crate::error::AppError;
use crate::types::chunks::SubagentDetail;
use async_trait::async_trait;

#[async_trait]
pub trait SubagentService: Send + Sync {
    /// 获取子 Agent 完整详情。
    ///
    /// 流水线：
    /// 1. 检查 DataCache 缓存（命中则直接返回）
    /// 2. 构建 JSONL 文件路径并解析
    /// 3. 通过 SubagentResolver 解析嵌套子 agent
    /// 4. 通过 ChunkBuilder 构建结构化详情
    /// 5. 写入缓存供后续查询
    async fn get_subagent_detail(
        &self,
        project_id: &str,
        session_id: &str,
        subagent_id: &str,
    ) -> Result<Option<SubagentDetail>, AppError>;
}
