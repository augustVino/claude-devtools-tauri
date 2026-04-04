//! 错误触发器检查器 —— 统一入口，重导出所有子模块符号。
//!
//! 提供以下功能：
//! - 检查 tool_result 触发器（error_status + content_match 模式）
//! - 检查 tool_use 触发器（基于工具输入内容的匹配）
//! - 检查 token_threshold 触发器（按 tool_use 的 token 计数）
//! - 验证项目范围（仓库过滤）
//!
//! 从 Electron `src/main/services/error/ErrorTriggerChecker.ts` 移植而来。

pub mod common;
pub mod repository_scope;
pub mod tool_result_checker;
pub mod tool_use_checker;
pub mod token_threshold_checker;

// =============================================================================
// 重导出所有公开符号（向后兼容）
// =============================================================================

// common
pub use common::{parse_timestamp_to_ms, truncate_content};

// repository_scope
pub use repository_scope::{matches_repository_scope, pre_resolve_repository_ids, RepositoryScopeTarget};

// tool_result_checker
pub use tool_result_checker::check_tool_result_trigger;

// tool_use_checker
pub use tool_use_checker::check_tool_use_trigger;

// token_threshold_checker
pub use token_threshold_checker::check_token_threshold_trigger;
