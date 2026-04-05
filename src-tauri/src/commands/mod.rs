//! Tauri IPC 命令处理模块。
//!
//! 本模块包含所有前端可调用的 Tauri command 函数，涵盖会话管理、
//! 项目浏览、配置读写、搜索、通知、子 Agent、窗口控制等功能。
//! 各子模块通过 `pub use` 统一导出，便于在 `lib.rs` 的
//! `generate_handler!` 宏中注册。

use std::sync::Arc;

use crate::infrastructure::{ConfigManager, DataCache};

pub mod window;
pub mod version;
pub mod sessions;
pub mod config;
pub mod search;
pub mod validation;
pub mod guards;
pub mod utility;
pub mod projects;
pub mod subagents;
pub mod notifications;
pub mod tray;
pub mod http_server;
pub mod context;
pub mod ssh;

#[allow(unused_imports)]
pub use window::*;
#[allow(unused_imports)]
pub use version::*;
#[allow(unused_imports)]
pub use sessions::*;
#[allow(unused_imports)]
pub use config::*;
#[allow(unused_imports)]
pub use search::*;
#[allow(unused_imports)]
pub use validation::*;
#[allow(unused_imports)]
pub use utility::*;
#[allow(unused_imports)]
pub use projects::*;
#[allow(unused_imports)]
pub use subagents::*;
#[allow(unused_imports)]
pub use notifications::*;
#[allow(unused_imports)]
pub use ssh::*;

/// 跨命令共享的应用状态。
///
/// 包含数据缓存和配置管理器，通过 `Arc<RwLock<AppState>>` 注入到各 Tauri command 中。
pub struct AppState {
    pub cache: DataCache,
    pub config_manager: Arc<ConfigManager>,
}

impl AppState {
    /// 创建应用状态。
    ///
    /// 必须传入外部共享的 `cache`，确保 AppState（IPC 命令层）与
    /// ServiceContext（文件监听器层）使用同一个缓存实例。
    pub fn new(config_manager: Arc<ConfigManager>, cache: DataCache) -> Self {
        Self { cache, config_manager }
    }

    /// 初始化应用状态，包括异步加载配置文件。
    #[allow(dead_code)]
    pub async fn initialize(&self) -> Result<(), String> {
        self.config_manager.initialize().await
            .map_err(|e| e.to_string())
    }
}
