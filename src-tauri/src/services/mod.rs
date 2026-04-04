//! Domain Service Layer.
//!
//! Encapsulates business logic shared between Tauri IPC commands and HTTP route handlers.
//! Each service is instantiated once in `lib.rs::setup()` and registered via `.manage()`.
//!
//! Modules:
//! - [`project_service`] — Project scanning, listing, repository grouping
//! - [`search_service`] — Session full-text search, ID lookup
//! - [`session_service`] — Session CRUD, detail building, metadata, waterfall

pub mod project_service;
pub mod search_service;
pub mod session_service;

pub use project_service::ProjectService;
pub use search_service::SearchService;
pub use session_service::SessionService;
