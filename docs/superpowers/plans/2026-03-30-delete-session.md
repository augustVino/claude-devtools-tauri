# Delete Session Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add "Delete Session" to the right-click context menu that deletes a session and all its associated files from the local filesystem.

**Architecture:** Rust backend command `delete_session` performs all file deletions (JSONL, subagents, tool-results, file-history, todos, debug, session-env, tasks, plans, security_warnings_state) plus config cleanup (pin/hide, sessions-index.json). Frontend wires the command through API client → Zustand store → SessionContextMenu with a danger-styled ConfirmDialog. SSH contexts are rejected with an error.

**Tech Stack:** Rust (tokio::fs, serde_json), Tauri IPC, React, Zustand, existing ConfirmDialog component

**Spec:** `docs/superpowers/specs/2026-03-30-delete-session-design.md`

---

### Task 1: Add `DeleteSessionResult` type to Rust domain types

**Files:**
- Modify: `src-tauri/src/types/domain.rs:315` (after `IpcResult`)

- [ ] **Step 1: Add the result struct**

Add after the `IpcResult<T>` struct:

```rust
/// 删除会话操作的结果统计。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeleteSessionResult {
    /// 是否删除了主 JSONL 文件
    pub main_file_deleted: bool,
    /// 成功删除的关联文件/目录数量
    pub associated_deleted: u32,
    /// 失败的删除操作数量（不影响整体成功）
    pub errors: u32,
}
```

- [ ] **Step 2: Verify compilation**

Run: `cd src-tauri && cargo check 2>&1 | tail -5`
Expected: no errors

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/types/domain.rs
git commit -m "feat(types): add DeleteSessionResult struct"
```

---

### Task 2: Implement `delete_session` Rust command

**Files:**
- Modify: `src-tauri/src/commands/sessions.rs` (add new command after `get_sessions_by_ids`)
- Modify: `src-tauri/src/utils/path_decoder.rs` (add `get_default_claude_base_path` if not already exported — it is, just need to add import)

- [ ] **Step 1: Add imports to sessions.rs**

At the top of `src-tauri/src/commands/sessions.rs`, add `get_default_claude_base_path` to the existing `utils` import on line 15:

Change:
```rust
use crate::utils::{decode_path, extract_base_dir, extract_project_name, get_projects_base_path};
```
To:
```rust
use crate::utils::{decode_path, extract_base_dir, extract_project_name, get_default_claude_base_path, get_projects_base_path};
```

Also add at the top of the file (after existing imports):
```rust
use crate::infrastructure::ContextManager;
use crate::types::domain::DeleteSessionResult;
```

- [ ] **Step 2: Add the `delete_session` command**

Add after the `get_sessions_by_ids` function (after line 348):

```rust
/// 删除指定会话及其所有关联文件。
///
/// 删除 JSONL 主文件、subagents、tool-results、file-history、todos、debug、
/// session-env、tasks、plans、security_warnings_state 等关联文件，
/// 同时清理 sessions-index.json 和配置中的 pin/hide 记录。
///
/// 仅支持本地上下文，SSH 远程上下文会返回错误。
#[command]
pub async fn delete_session(
    state: State<'_, Arc<RwLock<AppState>>>,
    context_manager: State<'_, Arc<RwLock<ContextManager>>>,
    project_id: String,
    session_id: String,
) -> Result<DeleteSessionResult, String> {
    // Validate session_id is a valid UUID
    if uuid::Uuid::parse_str(&session_id).is_err() {
        return Err(format!("Invalid session_id: '{}'", session_id));
    }

    // Reject SSH contexts — SFTP delete not yet supported
    {
        let mgr = context_manager.read().await;
        if let Some(active_ctx) = mgr.get_active() {
            let ctx = active_ctx.read().await;
            if ctx.context_type == crate::infrastructure::service_context::ContextType::Ssh {
                return Err("远程 session 暂不支持删除".to_string());
            }
        }
    }

    let claude_base = get_default_claude_base_path();
    let base_path = get_projects_base_path();
    let project_dir_name = extract_base_dir(&project_id);
    let project_dir = base_path.join(&project_dir_name);

    let mut main_file_deleted = false;
    let mut associated_deleted = 0u32;
    let mut errors = 0u32;

    // Helper: try delete, log on failure, count results
    async fn try_remove_file(path: &std::path::Path) -> bool {
        if tokio::fs::remove_file(path).await.is_ok() {
            true
        } else {
            false
        }
    }

    async fn try_remove_dir(path: &std::path::Path) -> bool {
        if tokio::fs::remove_dir_all(path).await.is_ok() {
            true
        } else {
            false
        }
    }

    // 1. Delete main JSONL file
    let jsonl_path = project_dir.join(format!("{}.jsonl", session_id));
    if jsonl_path.exists() {
        main_file_deleted = try_remove_file(&jsonl_path).await;
        if main_file_deleted {
            log::info!("Deleted session file: {}", jsonl_path.display());
        } else {
            log::warn!("Failed to delete session file: {}", jsonl_path.display());
            errors += 1;
        }
    }

    // 2. Delete session directory (subagents + tool-results)
    let session_dir = project_dir.join(&session_id);
    if session_dir.exists() {
        if try_remove_dir(&session_dir).await {
            associated_deleted += 1;
            log::info!("Deleted session directory: {}", session_dir.display());
        } else {
            errors += 1;
        }
    }

    // 3. Delete file-history
    let file_history_dir = claude_base.join("file-history").join(&session_id);
    if file_history_dir.exists() {
        if try_remove_dir(&file_history_dir).await {
            associated_deleted += 1;
        } else {
            errors += 1;
        }
    }

    // 4. Delete todos (glob match: {session_id}-*.json)
    let todos_dir = claude_base.join("todos");
    if let Ok(mut entries) = tokio::fs::read_dir(&todos_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&format!("{}-", session_id)) && name.ends_with(".json") {
                if try_remove_file(&entry.path()).await {
                    associated_deleted += 1;
                } else {
                    errors += 1;
                }
            }
        }
    }

    // 5. Delete debug logs (glob match: *{session_id}*.txt)
    let debug_dir = claude_base.join("debug");
    if let Ok(mut entries) = tokio::fs::read_dir(&debug_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.contains(&session_id) && name.ends_with(".txt") {
                if try_remove_file(&entry.path()).await {
                    associated_deleted += 1;
                } else {
                    errors += 1;
                }
            }
        }
    }

    // 6. Delete security_warnings_state
    let security_path = claude_base.join(format!("security_warnings_state_{}.json", session_id));
    if security_path.exists() {
        if try_remove_file(&security_path).await {
            associated_deleted += 1;
        } else {
            errors += 1;
        }
    }

    // 7. Delete session-env
    let session_env_dir = claude_base.join("session-env").join(&session_id);
    if session_env_dir.exists() {
        if try_remove_dir(&session_env_dir).await {
            associated_deleted += 1;
        } else {
            errors += 1;
        }
    }

    // 8. Delete tasks
    let tasks_dir = claude_base.join("tasks").join(&session_id);
    if tasks_dir.exists() {
        if try_remove_dir(&tasks_dir).await {
            associated_deleted += 1;
        } else {
            errors += 1;
        }
    }

    // 9. Delete plans (glob match: *{session_id}*.md)
    let plans_dir = claude_base.join("plans");
    if let Ok(mut entries) = tokio::fs::read_dir(&plans_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.contains(&session_id) && name.ends_with(".md") {
                if try_remove_file(&entry.path()).await {
                    associated_deleted += 1;
                } else {
                    errors += 1;
                }
            }
        }
    }

    // 10. Clean up sessions-index.json entry
    let index_path = project_dir.join("sessions-index.json");
    if index_path.exists() {
        if let Ok(content) = tokio::fs::read_to_string(&index_path).await {
            if let Ok(mut index) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(sessions_arr) = index.get_mut("sessions").and_then(|v| v.as_array_mut()) {
                    let before = sessions_arr.len();
                    sessions_arr.retain(|s| {
                        s.get("sessionId")
                            .or_else(|| s.get("session_id"))
                            .and_then(|v| v.as_str())
                            .map(|id| id != session_id)
                            .unwrap_or(true)
                    });
                    if sessions_arr.len() < before {
                        if let Ok(updated) = serde_json::to_string_pretty(&index) {
                            if tokio::fs::write(&index_path, updated).await.is_ok() {
                                log::info!("Updated sessions-index.json for deleted session {}", session_id);
                            } else {
                                errors += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    // 11. Clean up pin/hide records from ConfigManager
    {
        let app_state = state.read().await;
        app_state.config_manager.unpin_session(project_id.clone(), session_id.clone());
        app_state.config_manager.unhide_session(project_id.clone(), session_id.clone());
    }

    // Invalidate cache for this session
    {
        let app_state = state.read().await;
        app_state.cache.invalidate_session(&project_id, &session_id).await;
    }

    log::info!(
        "Session deleted: {} (main={}, associated={}, errors={})",
        session_id, main_file_deleted, associated_deleted, errors
    );

    Ok(DeleteSessionResult {
        main_file_deleted,
        associated_deleted,
        errors,
    })
}
```

- [ ] **Step 3: Verify `uuid` crate is in Cargo.toml**

Run: `grep '^uuid' src-tauri/Cargo.toml`
Expected: a line like `uuid = { version = "...", features = ["v4", "serde"] }`

If `uuid` is not present, check if there's another UUID validation approach used in the codebase. If needed, use a simple regex or string check instead:

```rust
// Fallback if uuid crate not available:
fn is_valid_uuid(s: &str) -> bool {
    s.len() == 36 && s.matches('-').count() == 4
}
```

- [ ] **Step 4: Verify compilation**

Run: `cd src-tauri && cargo check 2>&1 | tail -20`
Expected: no errors

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands/sessions.rs
git commit -m "feat(sessions): implement delete_session command"
```

---

### Task 3: Register `delete_session` in Tauri invoke handler

**Files:**
- Modify: `src-tauri/src/lib.rs:233` (in `invoke_handler![]`)

- [ ] **Step 1: Add to invoke handler**

Add `commands::sessions::delete_session` after `commands::sessions::get_waterfall_data` (line 233):

```rust
      commands::sessions::get_waterfall_data,
      commands::sessions::delete_session,
```

- [ ] **Step 2: Verify compilation**

Run: `cd src-tauri && cargo check 2>&1 | tail -5`
Expected: no errors

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat(tauri): register delete_session command"
```

---

### Task 4: Add `deleteSession` to frontend API client

**Files:**
- Modify: `src/api/tauriClient.ts` (add method in session commands section)

- [ ] **Step 1: Add the API method**

After the `getSessionsByIds` method (line 298), add:

```typescript
  readonly deleteSession = (
    projectId: string,
    sessionId: string,
  ): Promise<{ mainFileDeleted: boolean; associatedDeleted: number; errors: number }> =>
    invoke("delete_session", { projectId, sessionId });
```

- [ ] **Step 2: Verify no TypeScript errors**

Run: `pnpm build 2>&1 | tail -10`
Expected: no errors

- [ ] **Step 3: Commit**

```bash
git add src/api/tauriClient.ts
git commit -m "feat(api): add deleteSession to TauriAPIClient"
```

---

### Task 5: Add `deleteSession` action to session store slice

**Files:**
- Modify: `src/store/slices/sessionSlice.ts`

- [ ] **Step 1: Add to slice interface**

In the `SessionSlice` interface (after `pinMultipleSessions` around line 78), add:

```typescript
  /** Delete a session and all associated files */
  deleteSession: (projectId: string, sessionId: string) => Promise<void>;
```

- [ ] **Step 2: Implement the action**

In the slice creator (after `pinMultipleSessions` action, around line 520), add:

```typescript
  // Delete a session (non-optimistic — wait for backend confirmation)
  deleteSession: async (projectId: string, sessionId: string) => {
    const state = get();

    try {
      await api.deleteSession(projectId, sessionId);

      // Remove from sessions list
      set((prev) => ({
        sessions: prev.sessions.filter((s) => s.id !== sessionId),
        pinnedSessionIds: prev.pinnedSessionIds.filter((id) => id !== sessionId),
        hiddenSessionIds: prev.hiddenSessionIds.filter((id) => id !== sessionId),
        sidebarSelectedSessionIds: prev.sidebarSelectedSessionIds.filter((id) => id !== sessionId),
        // Clear selection if this was the active session
        ...(prev.selectedSessionId === sessionId
          ? { selectedSessionId: null, sessionDetail: null, sessionContextStats: null }
          : {}),
      }));

      // Refresh to update counts and pagination
      if (state.selectedProjectId) {
        void get().refreshSessionsInPlace(state.selectedProjectId);
      }
    } catch (error) {
      logger.error('deleteSession error:', error);
      throw error; // Re-throw so UI can show error
    }
  },
```

- [ ] **Step 3: Verify no TypeScript errors**

Run: `pnpm build 2>&1 | tail -10`
Expected: no errors

- [ ] **Step 4: Commit**

```bash
git add src/store/slices/sessionSlice.ts
git commit -m "feat(store): add deleteSession action to sessionSlice"
```

---

### Task 6: Add "Delete Session" to SessionContextMenu

**Files:**
- Modify: `src/components/sidebar/SessionContextMenu.tsx`
- Modify: `src/components/sidebar/SessionItem.tsx`

- [ ] **Step 1: Add props and handler to SessionContextMenu**

In `SessionContextMenu.tsx`:

1. Add `Trash2` to the lucide-react import on line 11:
```typescript
import { Check, ClipboardCopy, Eye, EyeOff, Pin, PinOff, Terminal, Trash2 } from 'lucide-react';
```

2. Add `onDelete` to the `SessionContextMenuProps` interface (after `onToggleHide`):
```typescript
  onDelete: () => void;
```

3. Destructure `onDelete` in the component props (after `onToggleHide`):
```typescript
  onDelete,
```

4. Update `menuHeight` from `290` to `330` to account for the new menu item.

5. Add a `handleDelete` callback (after `handleCopy` function, before `atMaxPanes`):
```typescript
  const handleDelete = async () => {
    const { confirm } = await import('@renderer/components/common/ConfirmDialog');
    const confirmed = await confirm({
      title: '删除 Session',
      message: '确定要删除此 session 吗？此操作不可撤销，将删除所有关联文件。',
      confirmLabel: '删除',
      cancelLabel: '取消',
      variant: 'danger',
    });
    if (confirmed) {
      onDelete();
    }
  };
```

6. Add the delete menu item before the closing `</div>` of the menu (after the "Copy Resume Command" item, after line 142):

```tsx
      <div className="mx-2 my-1 border-t" style={{ borderColor: 'var(--color-border)' }} />
      <MenuItem
        label="Delete Session"
        icon={<Trash2 className="size-4" style={{ color: 'rgb(248, 113, 113)' }} />}
        onClick={handleDelete}
        labelStyle={{ color: 'rgb(248, 113, 113)' }}
      />
```

7. Add `labelStyle` support to the `MenuItem` component. Add to the props type:
```typescript
  labelStyle?: React.CSSProperties;
```

And apply it in the label span:
```tsx
        {label}
```
Change to:
```tsx
        <span style={labelStyle}>{label}</span>
```

- [ ] **Step 2: Wire up in SessionItem.tsx**

In `SessionItem.tsx`:

1. Add `deleteSession` to the `useShallow` destructuring (alongside `togglePinSession` and `toggleHideSession`):
```typescript
  const {
    openTab,
    activeProjectId,
    selectSession,
    paneCount,
    splitPane,
    togglePinSession,
    toggleHideSession,
    deleteSession,
  } = useStore(
    useShallow((s) => ({
      openTab: s.openTab,
      activeProjectId: s.activeProjectId,
      selectSession: s.selectSession,
      paneCount: s.paneLayout.panes.length,
      splitPane: s.splitPane,
      togglePinSession: s.togglePinSession,
      toggleHideSession: s.toggleHideSession,
      deleteSession: s.deleteSession,
    }))
  );
```

2. Add the `onDelete` prop to `<SessionContextMenu>` (after `onToggleHide`):
```tsx
            onDelete={() => void deleteSession(activeProjectId, session.id)}
```

- [ ] **Step 3: Verify the app builds and runs**

Run: `pnpm build 2>&1 | tail -10`
Expected: no errors

Then visually verify:
Run: `pnpm tauri dev`
- Right-click a session in the sidebar
- Verify "Delete Session" appears at the bottom with red styling and trash icon
- Click it → confirm dialog appears with danger variant
- Cancel → nothing happens
- Confirm → session is deleted, list updates

- [ ] **Step 4: Commit**

```bash
git add src/components/sidebar/SessionContextMenu.tsx src/components/sidebar/SessionItem.tsx
git commit -m "feat(ui): add Delete Session to context menu with confirmation"
```

---

### Task 7: Final verification and edge cases

- [ ] **Step 1: Test with `pnpm tauri dev`**

Manual test checklist:
- [ ] Right-click session → "Delete Session" visible at bottom, red text, trash icon
- [ ] Click "Delete Session" → danger-styled confirm dialog appears
- [ ] Cancel confirm → nothing happens, context menu closes
- [ ] Confirm delete → session removed from list
- [ ] Delete currently selected session → selection cleared, detail view reset
- [ ] Delete pinned session → removed from pinned list
- [ ] Delete hidden session → removed from hidden list
- [ ] Delete multi-selected session → removed from multi-selection

- [ ] **Step 2: Run lint**

Run: `pnpm lint 2>&1 | tail -10`
Expected: no new errors

- [ ] **Step 3: Commit any fixes**

```bash
git add -A
git commit -m "fix: address lint issues in delete session implementation"
```
