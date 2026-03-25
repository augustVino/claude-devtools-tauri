/// Worktree Pattern Constants
///
/// Centralized worktree-related string literals to avoid duplication.
/// These are used in GitIdentityResolver for detecting worktree sources and paths.

// =============================================================================
// Directory Names
// =============================================================================

/// Standard git worktrees subdirectory
pub const WORKTREES_DIR: &str = "worktrees";

/// Workspaces directory (used by conductor)
pub const WORKSPACES_DIR: &str = "workspaces";

/// Tasks directory (used by auto-claude)
pub const TASKS_DIR: &str = "tasks";

// =============================================================================
// Worktree Source Identifiers
// =============================================================================

/// Cursor editor worktrees directory
pub const CURSOR_DIR: &str = ".cursor";

/// Vibe Kanban worktree source
pub const VIBE_KANBAN_DIR: &str = "vibe-kanban";

/// Conductor worktree source
pub const CONDUCTOR_DIR: &str = "conductor";

/// Auto-Claude worktree source
pub const AUTO_CLAUDE_DIR: &str = ".auto-claude";

/// 21st/1code worktree source
pub const TWENTYFIRST_DIR: &str = ".21st";

/// Claude Desktop worktrees directory
pub const CLAUDE_WORKTREES_DIR: &str = ".claude-worktrees";

/// ccswitch worktrees directory
pub const CCSWITCH_DIR: &str = ".ccswitch";