# macOS Menu Bar (Tray) Icon — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a macOS menu bar (tray) icon that appears when the Dock icon is hidden, with left-click window toggle and right-click context menu (window control, recent sessions, quit).

**Architecture:** `TrayIconManager` struct manages the tray lifecycle. It is registered as Tauri managed state. The existing `set_dock_visible` command delegates to it for coordinated dock/tray transitions. Window close is intercepted when dock is hidden to hide-to-tray instead.

**Tech Stack:** Tauri v2 `tauri::tray` module, `muda::Menu` (both transitive deps), `cocoa` + `objc` (existing macOS FFI)

**Spec:** `docs/superpowers/specs/2026-03-26-tray-menu-bar-design.md`

---

## File Structure

```
src-tauri/src/
├── commands/
│   ├── tray.rs              # NEW: TrayIconManager + tray commands
│   ├── window.rs            # MODIFY: set_dock_visible uses TrayIconManager
│   └── mod.rs               # MODIFY: add `pub mod tray;`
├── lib.rs                   # MODIFY: register TrayIconManager, startup tray, close interception
└── Cargo.toml               # MODIFY: add tray-icon feature to tauri dep
```

---

## Task 1: Enable tray-icon Feature

**Files:**
- Modify: `src-tauri/Cargo.toml:27`

- [ ] **Step 1: Add `tray-icon` feature to tauri dependency**

Change line 27 from:
```toml
tauri = { version = "2.10.3", features = ["macos-private-api"] }
```
to:
```toml
tauri = { version = "2.10.3", features = ["macos-private-api", "tray-icon"] }
```

- [ ] **Step 2: Verify compilation**

Run: `cd src-tauri && cargo check`
Expected: Compiles successfully (tray-icon feature activates `tauri::tray` module).

- [ ] **Step 3: Commit**

```bash
git add src-tauri/Cargo.toml
git commit -m "feat(tray): enable tray-icon feature on tauri crate"
```

---

## Task 2: Create TrayIconManager

**Files:**
- Create: `src-tauri/src/commands/tray.rs`
- Modify: `src-tauri/src/commands/mod.rs`

- [ ] **Step 1: Create `commands/tray.rs` with TrayIconManager struct**

```rust
use std::sync::Arc;
use tauri::{
    AppHandle, Emitter, Listener, Manager,
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tokio::sync::RwLock;

use super::sessions::AppState;
use crate::discovery::ProjectScanner;

pub struct TrayIconManager {
    icon: Option<tauri::tray::TrayIcon>,
    app_handle: AppHandle,
    dock_visible: bool,
}

impl TrayIconManager {
    pub fn new(app_handle: AppHandle) -> Self {
        Self {
            icon: None,
            app_handle,
            dock_visible: true,
        }
    }

    pub fn is_dock_visible(&self) -> bool {
        self.dock_visible
    }

    /// Create tray icon with context menu.
    /// Must be called on the main thread (macOS UI requirement).
    pub fn create(&mut self) -> Result<(), String> {
        if self.icon.is_some() {
            return Ok(()); // Already created
        }

        let menu = self.build_menu();
        let app_handle = self.app_handle.clone();

        let tray = TrayIconBuilder::new()
            .icon(app_handle.default_window_icon().unwrap().clone())
            .tooltip("claude-devtools")
            .icon_as_template(true)
            .menu(&menu)
            .on_tray_icon_event(move |_tray, event| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    Self::toggle_window(&app_handle);
                }
            })
            .build(&self.app_handle)
            .map_err(|e| format!("Failed to create tray icon: {e}"))?;

        self.icon = Some(tray);
        self.dock_visible = false;
        log::info!("Tray icon created");
        Ok(())
    }

    /// Destroy tray icon.
    pub fn destroy(&mut self) {
        if let Some(tray) = self.icon.take() {
            let _ = tray.set_visible(false);
            self.icon = None;
            log::info!("Tray icon destroyed");
        }
        self.dock_visible = true;
    }

    /// Toggle main window visibility.
    pub fn toggle_window(app: &AppHandle) {
        if let Some(window) = app.get_webview_window("main") {
            if window.is_visible().unwrap_or(false) {
                let _ = window.hide();
            } else {
                let _ = window.show();
                let _ = window.set_focus();
                #[cfg(target_os = "macos")]
                {
                    let _ = app.run_on_main_thread(|| {
                        unsafe {
                            let app = cocoa::appkit::NSApplication::sharedApplication(cocoa::base::nil);
                            let _: () = cocoa::appkit::NSApplication::activateIgnoringOtherApps_(app, true);
                        }
                    });
                }
            }
        }
    }

    /// Build the right-click context menu.
    fn build_menu(&self) -> muda::Menu {
        let menu = muda::Menu::new();

        // "显示/隐藏窗口" item
        let app_handle = self.app_handle.clone();
        let toggle_item = muda::MenuItem::new("显示/隐藏窗口", true, None, move || {
            Self::toggle_window(&app_handle);
        });
        let _ = menu.append(&toggle_item);

        // Separator
        let _ = menu.append(&muda::PredefinedMenuItem::separator());

        // "最近会话" submenu
        let recent_submenu = self.build_recent_sessions_submenu();
        let recent_item = muda::Submenu::new("最近会话", true, &recent_submenu);
        let _ = menu.append(&recent_item);

        // Separator
        let _ = menu.append(&muda::PredefinedMenuItem::separator());

        // "退出" item
        let app_handle = self.app_handle.clone();
        let quit_item = muda::MenuItem::new("退出", true, None, move || {
            app_handle.exit(0);
        });
        let _ = menu.append(&quit_item);

        menu
    }

    /// Build the recent sessions submenu (max 5 entries).
    fn build_recent_sessions_submenu(&self) -> muda::Menu {
        let submenu = muda::Menu::new();
        let scanner = ProjectScanner::new();
        let projects = scanner.scan();

        let recent: Vec<_> = projects
            .into_iter()
            .filter(|p| p.most_recent_session.is_some())
            .take(5)
            .collect();

        if recent.is_empty() {
            let item = muda::MenuItem::new("无最近会话", false, None, || {});
            let _ = submenu.append(&item);
            return submenu;
        }

        for project in recent {
            let label = project.name.clone();
            let project_id = project.id.clone();
            let session_id = project.sessions.last().cloned().unwrap_or_default();
            let app_handle = self.app_handle.clone();

            let item = muda::MenuItem::new(&label, true, None, move || {
                // Emit event for frontend to navigate
                let payload = OpenSessionPayload {
                    project_id: project_id.clone(),
                    session_id: session_id.clone(),
                };
                let _ = app_handle.emit("tray:open-session", &payload);

                // Show and focus the window
                Self::toggle_window(&app_handle);
            });
            let _ = submenu.append(&item);
        }

        submenu
    }

    /// Rebuild the tray icon with updated menu (call from file watcher when sessions change).
    /// Since Tauri v2 TrayIcon does not expose set_menu(), we destroy and recreate.
    pub fn refresh_menu(&mut self) -> Result<(), String> {
        let was_dock_visible = self.dock_visible;
        self.icon = None; // Drop existing without setting dock_visible
        self.create()?;   // Rebuild with fresh menu
        self.dock_visible = was_dock_visible; // Restore state
        Ok(())
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenSessionPayload {
    project_id: String,
    session_id: String,
}
```

- [ ] **Step 2: Register tray module in `commands/mod.rs`**

Add after line 1:
```rust
pub mod tray;
```

- [ ] **Step 3: Verify compilation**

Run: `cd src-tauri && cargo check`
Expected: Compiles successfully.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/commands/tray.rs src-tauri/src/commands/mod.rs
git commit -m "feat(tray): add TrayIconManager with window toggle and context menu"
```

---

## Task 3: Wire set_dock_visible to TrayIconManager

**Files:**
- Modify: `src-tauri/src/commands/window.rs:49-110`

- [ ] **Step 1: Modify `set_dock_visible` to use TrayIconManager**

Replace the macOS `set_dock_visible` function (lines 49-103) with:

```rust
#[cfg(target_os = "macos")]
#[command]
pub async fn set_dock_visible(
    tray: tauri::State<'_, std::sync::Mutex<TrayIconManager>>,
    state: tauri::State<'_, Arc<RwLock<AppState>>>,
    visible: bool,
) -> Result<(), String> {
    use cocoa::appkit::{NSApplication, NSApplicationActivationPolicy};
    use cocoa::base::nil;
    use cocoa::foundation::NSString;
    use objc::{runtime::Object, *};

    unsafe {
        let app = NSApplication::sharedApplication(nil);
        if visible {
            app.setActivationPolicy_(
                NSApplicationActivationPolicy::NSApplicationActivationPolicyRegular,
            );
            // Restore app icon (macOS quirk: icon lost after Accessory → Regular)
            let bundle: *mut Object = msg_send![class!(NSBundle), mainBundle];
            let icon_key = NSString::alloc(nil).init_str("CFBundleIconFile");
            let icon_name: *mut Object = msg_send![bundle, objectForInfoDictionaryKey: icon_key];
            if !icon_name.is_null() {
                let icon_cstr: *const std::os::raw::c_char = msg_send![icon_name, UTF8String];
                let icon_str = std::ffi::CStr::from_ptr(icon_cstr);
                let icon_name = icon_str.to_str().unwrap_or("icon");
                let bundle_path: *mut Object = msg_send![bundle, bundlePath];
                let path_cstr: *const std::os::raw::c_char = msg_send![bundle_path, UTF8String];
                let path_str = std::ffi::CStr::from_ptr(path_cstr);
                let bundle_path_str = path_str.to_str().unwrap_or(".");
                let icon_path = format!("{bundle_path_str}/Contents/Resources/{icon_name}");

                let ns_image: *mut Object = msg_send![class!(NSImage), alloc];
                let ns_path = NSString::alloc(nil).init_str(&icon_path);
                let image: *mut Object = msg_send![ns_image, initWithContentsOfFile: ns_path];
                if !image.is_null() {
                    let _: () = msg_send![app, setApplicationIconImage: image];
                }
            }
            // Remove tray (no longer needed)
            tray.lock().map_err(|e| e.to_string())?.destroy();
        } else {
            // Create tray FIRST (ensures user always has an entry point)
            tray.lock().map_err(|e| e.to_string())?.create()?;
            // Then hide dock
            app.setActivationPolicy_(
                NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
            );
        }
    }
    // Persist to config
    let app_state = state.read().await;
    app_state
        .config_manager
        .update_config(
            "general",
            serde_json::json!({ "showDockIcon": visible }),
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}
```

Also update the import at line 5 to include TrayIconManager:
```rust
use super::{sessions::AppState, tray::TrayIconManager};
```

- [ ] **Step 2: Verify compilation**

Run: `cd src-tauri && cargo check`
Expected: Compiles successfully.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/commands/window.rs
git commit -m "feat(tray): wire set_dock_visible to TrayIconManager"
```

---

## Task 4: Register TrayIconManager in lib.rs + Close Interception

**Files:**
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Import TrayIconManager and register as managed state**

After line 18 (`use infrastructure::{ConfigManager, FileWatcher, NotificationManager};`), add:
```rust
use commands::tray::TrayIconManager;
```

After line 34 (`.manage(app_state.clone())`), add:
```rust
.manage(std::sync::Mutex::new(TrayIconManager::new(app.handle().clone())))
```

- [ ] **Step 2: Update startup dock-hiding to use TrayIconManager**

Replace the macOS dock-hiding block (lines 47-63) with:

```rust
// macOS: Create tray and hide Dock icon if config says so
#[cfg(target_os = "macos")]
{
    let hide_dock = {
        let state_guard = state.blocking_read();
        !state_guard.config_manager.get_config().general.show_dock_icon
    };
    if hide_dock {
        // Create tray FIRST, then hide dock
        let tray = app.state::<std::sync::Mutex<TrayIconManager>>();
        let _ = tray.lock().map(|mut t| t.create());
        use cocoa::appkit::{NSApplication, NSApplicationActivationPolicy};
        use cocoa::base::nil;
        unsafe {
            let ns_app = NSApplication::sharedApplication(nil);
            ns_app.setActivationPolicy_(
                NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
            );
        }
    }
}
```

- [ ] **Step 3: Add window close interception**

After the `.manage(std::sync::Mutex::new(TrayIconManager::new(...)))` line, add before `.setup(move |app| {`:

```rust
.on_window_event(|window, event| {
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        #[cfg(target_os = "macos")]
        {
            let tray = window.app_handle().state::<std::sync::Mutex<TrayIconManager>>();
            if let Ok(tray) = tray.lock() {
                if !tray.is_dock_visible() {
                    let _ = window.hide();
                    api.prevent_close();
                }
            }
        }
    }
})
```

Note: `.on_window_event()` must be placed on the `tauri::Builder` chain, between `.manage()` and `.setup()`.

- [ ] **Step 4: Verify compilation**

Run: `cd src-tauri && cargo check`
Expected: Compiles successfully.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "feat(tray): register TrayIconManager, add close interception"
```

---

## Task 5: Manual Verification

- [ ] **Step 1: Build and launch the app**

Run: `cd /Users/liepin/Documents/github/claude-devtools-tauri && pnpm tauri dev`

- [ ] **Step 2: Verify tray does NOT appear when dock is visible**

Expected: No menu bar icon visible. Dock icon is present.

- [ ] **Step 3: Toggle "Show dock icon" off in Settings**

Expected: Dock icon disappears, menu bar tray icon appears. Window remains visible.

- [ ] **Step 4: Left-click tray icon**

Expected: Window hides. Left-click again: window shows and app activates.

- [ ] **Step 5: Right-click tray icon**

Expected: Context menu shows "显示/隐藏窗口", "最近会话" submenu (with sessions or "无最近会话"), "退出".

- [ ] **Step 6: Click close button while dock is hidden**

Expected: Window hides (not app exits). Tray icon remains. App still running.

- [ ] **Step 7: Toggle "Show dock icon" back on**

Expected: Dock icon reappears with correct app icon. Tray icon disappears.

- [ ] **Step 8: Click "退出" in tray menu**

Expected: App exits completely.

- [ ] **Step 9: Restart with `showDockIcon: false` config**

Run: `pnpm tauri dev` again.
Expected: App launches with tray icon visible, no dock icon. Window visible if not `--minimized`.

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "feat(tray): manual verification passed"
```
