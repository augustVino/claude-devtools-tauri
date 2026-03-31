//! Discovery module - Project and session discovery services.
//!
//! This module contains services for discovering and listing projects and sessions:
//! - `ProjectScanner` - Scans ~/.claude/projects/ directory
//! - `SessionSearcher` - Searches sessions for query strings
//! - `WorktreeGrouper` - Groups projects by git repository
//! - `SubagentResolver` - Resolves subagent files and links them to Task calls

pub mod project_scanner;
pub mod session_searcher;
pub mod subagent_resolver;
pub mod subproject_registry;
pub mod worktree_grouper;

pub use project_scanner::*;
pub use session_searcher::*;
pub use subagent_resolver::*;
pub use subproject_registry::*;
pub use worktree_grouper::*;