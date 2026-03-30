# Delete Session Design

## Summary

Add a "Delete Session" action to the right-click context menu in the sessions list. When triggered, it deletes the session's main JSONL file and all associated files (subagents, tool-results, file-history, todos, debug, session-env, tasks, plans, security_warnings_state), cleans up config entries (pin/hide), and updates sessions-index.json.

## Constraints

- **Local only**: The app's SSH remote feature is Phase 1 stub (SFTP not implemented). Delete is only supported for local sessions. If the active context is SSH, the command returns an error.
- **Path resolution**: Uses the same `get_default_claude_base_path()` as all existing session commands. The `claude_root_path` config field exists but is not wired up in the codebase — delete follows the same pattern as existing commands.

## UI

- New "Delete Session" menu item at the bottom of `SessionContextMenu`, separated by a divider
- Red text (danger style) to visually distinguish from other actions
- Uses existing `ConfirmDialog` component with `variant: 'danger'` for confirmation
- Confirmation message: "确定要删除此 session 吗？此操作不可撤销，将删除所有关联文件。"

## Files to Delete

When deleting session `{session_id}` from project `{project_dir}`:

1. `{project_dir}/{session_id}.jsonl` — main chat file
2. `{project_dir}/{session_id}/` — directory containing `subagents/` and `tool-results/`
3. `{claude_base}/file-history/{session_id}/`
4. `{claude_base}/todos/{session_id}-*.json` — glob match
5. `{claude_base}/debug/*{session_id}*.txt` — glob match
6. `{claude_base}/security_warnings_state_{session_id}.json`
7. `{claude_base}/session-env/{session_id}/`
8. `{claude_base}/tasks/{session_id}/`
9. `{claude_base}/plans/*{session_id}*.md` — glob match

Also:
- Remove entry from `{project_dir}/sessions-index.json` (if file exists)
- Remove pin/hide records from ConfigManager

**NOT deleted** (to avoid breaking other sessions):
- `{claude_base}/agents/*/memory-project.md` — may be shared across sessions
- `{claude_base}/agents/*/memory-user.md` — global user memory

## Implementation

### Rust Backend (`commands/sessions.rs`)

New `delete_session` command:
- Input: `project_id: String`, `session_id: String`
- Validate session_id is a valid UUID format
- Check active context via ContextManager — reject if SSH context
- Construct file paths using `get_default_claude_base_path()` and `get_projects_base_path()`
- Delete files/directories using `tokio::fs::remove_file()` and `tokio::fs::remove_dir_all()`
- Clean up sessions-index.json entry
- Remove pin/hide via ConfigManager
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
- If deleted session was multi-selected, remove from `sidebarSelectedSessionIds`

### UI (`SessionContextMenu.tsx`)

Add "Delete Session" menu item with:
- Red text color via inline style (consistent with ConfirmDialog danger variant)
- `Trash2` icon from lucide-react
- Separator divider above it
- onClick handler: show ConfirmDialog with danger variant, then call store action

### UI (`SessionItem.tsx`)

Pass new `onDelete` callback prop to `SessionContextMenu`.

## Error Handling

- If some files fail to delete, log warnings but report overall success
- If the main .jsonl file doesn't exist, consider the session already deleted (idempotent)
- Validation error on invalid session_id returns error to frontend
- SSH context returns error: "远程 session 暂不支持删除"
