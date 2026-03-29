//! Tauri IPC 命令处理模块。
//!
//! 本模块包含所有前端可调用的 Tauri command 函数，涵盖会话管理、
//! 项目浏览、配置读写、搜索、通知、子 Agent、窗口控制等功能。
//! 各子模块通过 `pub use` 统一导出，便于在 `lib.rs` 的
//! `generate_handler!` 宏中注册。

pub mod window;
pub mod version;
pub mod sessions;
pub mod config;
pub mod search;
pub mod validation;
pub mod utility;
pub mod projects;
pub mod subagents;
pub mod notifications;
pub mod updater;
pub mod tray;
pub mod http_server;
pub mod context;

pub use window::*;
pub use version::*;
pub use sessions::*;
pub use config::*;
pub use search::*;
pub use validation::*;
pub use utility::*;
pub use projects::*;
pub use subagents::*;
pub use notifications::*;
pub use sessions::AppState;
