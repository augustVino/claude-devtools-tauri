//! 错误检测模块。
//!
//! 负责从 JSONL 会话文件中检测错误并触发通知，包含以下子模块：
//!
//! - [`error_detector`] — 错误检测主编排器，协调触发器检查与去重
//! - [`error_message_builder`] — 从工具结果中提取错误文本并构建 [`DetectedError`] 实例
//! - [`trigger`] — 针对不同触发器类型（tool_result、tool_use、token_threshold）的消息检查
//! - [`error_trigger_tester`] — 在历史会话数据上测试触发器配置
//! - [`trigger_matcher`] — 触发器的正则模式匹配、忽略模式检查等通用工具

pub mod error_detector;
pub mod error_message_builder;
pub mod error_trigger_tester;
pub mod trigger;
pub mod trigger_matcher;

// 向后兼容：通过 trigger 子模块重导出所有符号
pub use trigger::*;

pub mod app_error;
pub use app_error::AppError;
