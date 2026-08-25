//! Agent 适配层 —— 多 agent 会话支持的统一入口。
//!
//! # 架构定位（防腐层 / Anti-Corruption Layer）
//!
//! 各 agent 工具（Claude Code / Codex CLI / OpenCode / Pi / dsh）的本地会话
//! 格式互不兼容。本模块把「格式知识」收敛到各家的 adapter 内部，向下游
//! （分类、Chunk 构建、瀑布图、上下文追踪、前端渲染）只暴露统一的
//! [`ParsedMessage`] 中间表示 —— 下游管线不感知会话来自哪家 agent。
//!
//! # Adapter 输出契约
//!
//! adapter 输出的 `ParsedMessage` 必须满足以下字段契约。契约分三类：
//!
//! ## 1. 中立字段（每家 adapter 必须尽力填充）
//!
//! | 字段 | 语义 |
//! |------|------|
//! | `uuid` / `timestamp` / `role` / `model` / `usage` | 会话内稳定 id、ISO 8601 时间、角色、模型、token 用量 |
//! | `content` | **块协议**：`text` / `thinking` / `tool_use` / `tool_result` 四种块的 JSON 数组（Anthropic 块协议即本项目的通用语；各家原生块类型映射到此协议后，下游 tool 配对、瀑布、上下文追踪即可直接工作） |
//! | `tool_calls` / `tool_results` | 从 content 抽取的结构化工具调用/结果（`extract_tool_calls` 语义） |
//! | `cwd` / `git_branch` | 会话所属项目路径与分支 |
//!
//! ## 2. 泛化语义字段（名字沿自 Claude，语义中立；各家用自家证据填充）
//!
//! | 字段 | 中立语义 | 各家证据示例 |
//! |------|---------|-------------|
//! | `is_meta` | 非真人输入（注入上下文/环境回显），下游折叠展示 | Claude: `isMeta` 列；Codex: 内容前缀（`<environment_context` 等）；OpenCode: `synthetic` 列；dsh: `source.kind != "user"` |
//! | `is_compact_summary` | 上下文压缩产生的摘要消息 | Claude: `isCompactSummary`；Codex: `compacted` 行 |
//! | `is_sidechain` | 子代理（subagent）消息 | Claude: `isSidechain`；OpenCode: `parent_id`；dsh: `origin=subagent` |
//!
//! ## 3. Claude 特有字段（其他 adapter 一律输出 None / 空值，下游遇空短路）
//!
//! `request_id`（含**去重**：Claude 流式写入的重复行必须在 adapter 内按
//! requestId 去重后才输出 —— 这是 Claude 语义，不得泄漏为下游职责）、
//! `parent_uuid`、`source_tool_use_id`、`source_tool_assistant_uuid`、
//! `tool_use_result`、`user_type`、`agent_id`。
//!
//! # 新增 agent 的检查单
//!
//! 1. 在 [`crate::types::domain::AgentKind`] 加变体（注意 `AgentKind::ALL`
//!    顺序即前端展示顺序）；
//! 2. 新建 `agents/<name>.rs` 实现 [`AgentAdapter`]，**在 parse 阶段消化
//!    自家噪声**（注入内容 → `is_meta`，压缩 → `is_compact_summary`），
//!    不得依赖下游 classifier 认识你的原生格式；
//! 3. 在 [`create_adapters`] 注册；
//! 4. 不认识的行不要静默吞掉 —— 至少 `log::warn!`（schema 漂移金丝雀）。
//!
//! # 当前接线状态
//!
//! - P0：`session_service` 的会话解析已通过 [`adapter_for_path`] dispatch；
//!   `project_scanner` / `file_watcher` 仍是 Claude 单一实现，随第二家
//!   adapter（P1+）接入时改造为遍历 registry。

mod claude;

use std::path::Path;
use std::sync::{Arc, OnceLock};

use crate::infrastructure::fs_provider::FsProvider;
use crate::types::domain::AgentKind;
use crate::types::messages::ParsedMessage;

pub use claude::ClaudeAdapter;

/// 单个 agent 工具的会话数据适配器。
///
/// 实现方负责：把该 agent 的本地会话文件解析为符合模块契约的
/// [`ParsedMessage`] 列表。所有文件读取必须走 `FsProvider`
/// （SSH 模式下本地 `std::fs` 读不到远程文件）。
pub trait AgentAdapter: Send + Sync {
    /// 本 adapter 服务的 agent 类型。
    fn kind(&self) -> AgentKind;

    /// 本家会话数据根目录（如 `~/.claude/projects`）。
    ///
    /// 是路径知识的唯一事实源：路径 → agent 的归属判定从它派生。
    /// 注意：返回的是本地路径视角；SSH 上下文的根由 ServiceContext
    /// 持有的 projects_dir 决定，不在此体现。
    fn data_roots(&self) -> Vec<std::path::PathBuf>;

    /// 解析单个会话文件 → 统一中间表示。
    ///
    /// 契约：
    /// - 输出必须已按模块文档完成 Claude 特有语义的消化（如 requestId 去重）；
    /// - 文件不存在 / 读取失败 → 返回空 `ParsedSession`（与既有
    ///   `parse_session_file_with_provider` 的容错语义一致，不向上抛错）。
    fn parse_messages(&self, path: &Path, fs: &dyn FsProvider) -> Vec<ParsedMessage>;
}

/// 全量 adapter 注册表（进程级单例）。
///
/// 顺序即 `AgentKind::ALL` 的展示序。P0 仅 Claude 一家；每新增一家 agent
/// 在此追加，dispatch 逻辑（[`adapter_for_path`]）随之自动覆盖。
pub fn create_adapters() -> Vec<Arc<dyn AgentAdapter>> {
    vec![Arc::new(ClaudeAdapter::new())]
}

fn registry() -> &'static [Arc<dyn AgentAdapter>] {
    static REGISTRY: OnceLock<Vec<Arc<dyn AgentAdapter>>> = OnceLock::new();
    REGISTRY.get_or_init(create_adapters)
}

/// 路径 → 适配器。P0 单 adapter 阶段唯一正确行为是恒返 Claude：
/// 数据根前缀匹配依赖本地 home 推导，在 SSH 上下文必然失配，
/// 等第二家 agent 接入时连同各上下文的根判定一并引入。
pub fn adapter_for_path(_path: &Path) -> &'static dyn AgentAdapter {
    registry()[0].as_ref()
}

/// 路径 → agent 类型（[`adapter_for_path`] 的便捷封装）。
pub fn agent_for_path(path: &Path) -> AgentKind {
    adapter_for_path(path).kind()
}

/// 路径 → adapter 解析 → 聚合为 [`crate::parsing::ParsedSession`]。
///
/// 是 session_service 各解析入口的统一替换点（对齐旧
/// `parse_session_file_with_provider` 的输出形态），第二家 agent 接入后
/// 此处自动按路径分派。
pub fn parse_session_for(path: &Path, fs: &dyn FsProvider) -> crate::parsing::ParsedSession {
    let messages = adapter_for_path(path).parse_messages(path, fs);
    crate::parsing::process_messages(&messages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_all_registered_kinds_in_order() {
        let adapters = create_adapters();
        assert_eq!(adapters.len(), 1);
        assert_eq!(adapters[0].kind(), AgentKind::ClaudeCode);
    }

    #[test]
    fn adapter_for_path_dispatches_to_registry() {
        let any_path = Path::new("/Users/x/.claude/projects/-proj/s1.jsonl");
        assert_eq!(adapter_for_path(any_path).kind(), AgentKind::ClaudeCode);
        assert_eq!(agent_for_path(any_path), AgentKind::ClaudeCode);
    }
}
