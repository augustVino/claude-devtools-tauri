//! 基础设施层 — 提供配置管理、数据缓存、文件监听和通知管理等核心服务。

pub mod config_manager;
pub mod context_manager;
pub mod context_rebuild;
pub mod data_cache;
pub mod file_watcher;
pub mod fs_provider;
pub mod notification_manager;
pub mod service_context;
pub mod ssh_auth;
pub mod ssh_config_parser;
pub mod ssh_connection_manager;
pub mod ssh_exec;
pub mod ssh_fs_provider;
pub mod session_repository;
pub mod trigger_manager;

pub use config_manager::ConfigManager;
pub use context_manager::{ContextInfo, ContextManager};
pub use data_cache::DataCache;
pub use file_watcher::FileWatcher;
pub use fs_provider::{FsDirent, FsProvider, FsStatResult, LocalFsProvider};
pub use notification_manager::NotificationManager;
pub use ssh_config_parser::SshConfigParser;
pub use ssh_connection_manager::SshConnectionManager;
pub use session_repository::{DeleteFilesResult, SessionFileItem, SessionRepository};
#[allow(unused_imports)]
pub use service_context::{ContextType, ServiceContext, ServiceContextConfig};
#[allow(unused_imports)]
pub use trigger_manager::TriggerManager;
