//! 基础设施层 — 提供配置管理、数据缓存、文件监听和通知管理等核心服务。

pub mod app_bootstrap;
pub mod config;
pub mod config_validator;
pub mod context_manager;
pub mod context_rebuild;
pub mod data_cache;
pub mod file_watcher;
pub mod fs_provider;
pub mod local_session_repository;
pub mod git_facts_cache;
pub mod listing_cache;
pub mod notification;
pub mod service_context;
pub mod session_repository;
pub mod ssh_auth;
pub mod ssh_config_parser;
pub mod ssh_connection;
pub mod ssh_exec;
pub mod ssh_fs_provider;
pub mod trigger_manager;
pub mod watcher_orchestrator;

pub use config::ConfigManager;
pub use context_manager::ContextManager;
pub use data_cache::DataCache;
pub use file_watcher::FileWatcher;
pub use fs_provider::{FsProvider, LocalFsProvider};
pub use notification::NotificationManager;
#[allow(unused_imports)]
pub use service_context::{ContextType, ServiceContext, ServiceContextConfig};
pub use ssh_connection::SshConnectionManager;
#[allow(unused_imports)]
pub use trigger_manager::TriggerManager;
