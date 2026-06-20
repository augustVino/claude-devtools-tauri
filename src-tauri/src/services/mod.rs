//! Domain Service Layer.
//!
//! Encapsulates business logic shared between Tauri IPC commands and HTTP route handlers.
//! Each service is instantiated once in `lib.rs::setup()` and registered via `.manage()`.
//!
//! Modules:
//! - [`project_service`] — Project scanning, listing, repository grouping
//! - [`search_service`] — Session full-text search, ID lookup
//! - [`session_service`] — Session CRUD, detail building, metadata, waterfall
//! - [`memory_service`] — Memory viewer: read Claude memory directory and MEMORY.md index
//!
//! Trait modules (Phase E):
//! - [`project_service_trait`] — ProjectService trait definition
//! - [`search_service_trait`] — SearchService + SearchServiceRebuild trait definitions
//! - [`session_service_trait`] — SessionService trait definition
//! - [`subagent_service_trait`] — SubagentService trait definition (Phase B-B2)
//! - [`ssh_service_trait`] — SshService trait definition (Phase B-B2)
//! - [`config_service_trait`] — ConfigService trait definition (Phase B-B3)
//! - [`memory_service_trait`] — MemoryService trait definition

pub mod config_service_impl;
pub mod config_service_trait;
pub mod memory_service;
pub mod memory_service_trait;
pub mod project_service;
pub mod project_service_trait;
pub mod search_service;
pub mod search_service_trait;
pub mod session_service;
pub mod session_service_trait;
pub mod ssh_service;
pub mod ssh_service_trait;
pub mod subagent_service;
pub mod subagent_service_trait;

// Re-exports: traits take original name, Impl gets suffix
pub use config_service_impl::ConfigServiceImpl;
pub use config_service_trait::ConfigService;
pub use memory_service::MemoryServiceImpl;
pub use memory_service_trait::MemoryService;
pub use project_service::ProjectServiceImpl;
pub use project_service_trait::ProjectService;
pub use search_service::SearchServiceImpl;
pub use search_service_trait::{SearchService, SearchServiceFull, SearchServiceRebuild};
pub use session_service::SessionServiceImpl;
pub use session_service_trait::SessionService;
pub use ssh_service::SshServiceImpl;
pub use ssh_service_trait::SshService;
pub use subagent_service::SubagentServiceImpl;
pub use subagent_service_trait::SubagentService;
