# Delete Session Design

## Summary

Add a "Delete Session" action to the right-click context menu in the sessions list. When triggered, it deletes the session's main JSONL file and all associated files (subagents, tool-results, file-history, todos, debug, session-env, tasks, plans, security_warnings_state), cleans up config entries (pin/hide), and updates sessions-index.json.

## UI

- New "Delete Session" menu item at the bottom of `SessionContextMenu`, separated by a divider
- Red text (danger style) to visually distinguish from other actions
- Confirmation dialog before deletion: "确定要删除此 session 吗？此操作不可撤销，将删除所有关联文件。"

## Files to Delete

When deleting session `{session_id}` from project `{project_dir}`:

1. `{project_dir}/{session_id}.jsonl` — main chat file
2. `{project_dir}/{session_id}/` — directory containing `subagents/` and `tool-results/`
3. `~/.claude/file-history/{session_id}/`
4. `~/.claude/todos/{session_id}-*.json` — glob match
5. `~/.claude/debug/*{session_id}*.txt` — glob match
6. `~/.claude/security_warnings_state_{session_id}.json`
7. `~/.claude/session-env/{session_id}/`
8. `~/.claude/tasks/{session_id}/`
9. `~/.claude/plans/*{session_id}*.md` — glob match

Also:
- Remove entry from `{project_dir}/sessions-index.json` (if file exists)
- Remove pin/hide records from ConfigManager

**NOT deleted** (to avoid breaking other sessions):
- `~/.claude/agents/*/memory-project.md` — may be shared across sessions
- `~/.claude/agents/*/memory-user.md` — global user memory

## Implementation

### Rust Backend (`commands/sessions.rs`)

New `delete_session` command:
- Input: `project_id: String`, `session_id: String`
- Validate session_id is a valid UUID
- Construct file paths using `get_projects_base_path()` and the claude base dir
- Delete files/directories using `tokio::fs::remove_file()` and `tokio::fs::remove_dir_all()`
- Return `Result<DeleteSessionResult, String>` with deletion statistics
- All file operations are best-effort (log warnings for missing files, don't fail)

### Rust Backend (`lib.rs`)

Register `delete_session` in `invoke_handler![]`.

### Frontend API (`api/tauriClient.ts`)

New `deleteSession(projectId: string, sessionId: string): Promise<DeleteSessionResult>` method.

### Store (`sessionSlice.ts`)

New `deleteSession(projectId, sessionId)` action:
- Call API
- On success: remove from `sessions` array, clear from `pinnedSessionIds`/`hiddenSessionIds`
- If deleted session was selected, clear `selectedSessionId`
- Trigger `refreshSessionsInPlace` to update the list

### UI (`SessionContextMenu.tsx`)

Add "Delete Session" menu item with:
- `text-danger` class for red styling
- Separator divider above it
- onClick handler: show confirmation dialog, then call store action

## Error Handling

- If some files fail to delete, log warnings but report overall success
- If the main .jsonl file doesn't exist, consider the session already deleted (idempotent)
- Validation error on invalid session_id returns error to frontend
