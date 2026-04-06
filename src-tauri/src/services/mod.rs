//! Domain Service Layer.
//!
//! Encapsulates business logic shared between Tauri IPC commands and HTTP route handlers.
//! Each service is instantiated once in `lib.rs::setup()` and registered via `.manage()`.
//!
//! Modules:
//! - [`project_service`] — Project scanning, listing, repository grouping
//! - [`search_service`] — Session full-text search, ID lookup
//! - [`session_service`] — Session CRUD, detail building, metadata, waterfall
//!
//! Trait modules (Phase E):
//! - [`project_service_trait`] — ProjectService trait definition
//! - [`search_service_trait`] — SearchService + SearchServiceRebuild trait definitions
//! - [`session_service_trait`] — SessionService trait definition
//! - [`subagent_service_trait`] — SubagentService trait definition (Phase B-B2)
//! - [`ssh_service_trait`] — SshService trait definition (Phase B-B2)

pub mod project_service;
pub mod search_service;
pub mod session_service;
pub mod project_service_trait;
pub mod search_service_trait;
pub mod session_service_trait;
pub mod subagent_service_trait;
pub mod subagent_service;
pub mod ssh_service_trait;
pub mod ssh_service;

// Re-exports: traits take original name, Impl gets suffix
pub use session_service_trait::SessionService;
pub use search_service_trait::{SearchService, SearchServiceFull, SearchServiceRebuild};
pub use project_service_trait::ProjectService;
pub use subagent_service_trait::SubagentService;
pub use ssh_service_trait::SshService;
pub use session_service::SessionServiceImpl;
pub use search_service::SearchServiceImpl;
pub use project_service::ProjectServiceImpl;
pub use subagent_service::SubagentServiceImpl;
pub use ssh_service::SshServiceImpl;
