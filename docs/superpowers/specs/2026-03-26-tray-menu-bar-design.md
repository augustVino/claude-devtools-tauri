# macOS Menu Bar (Tray) Icon — Design Spec

**Goal:** Add a system tray/menu bar icon that appears when the Dock icon is hidden, ensuring the app remains accessible. Left-click toggles the main window; right-click shows a context menu with window control, recent sessions, and quit.

**Scope:** macOS only. No changes to Electron version. No new Cargo dependencies (`tray-icon` and `muda` are already transitive dependencies of Tauri v2).

---

## Requirements

| # | Requirement | Priority |
|---|-------------|----------|
| 1 | Tray icon appears automatically when Dock icon is hidden | Must |
| 2 | Tray icon disappears automatically when Dock icon is restored | Must |
| 3 | Left-click tray icon toggles main window show/hide | Must |
| 4 | Right-click shows context menu (window toggle, recent sessions, quit) | Must |
| 5 | Close button hides window to tray when Dock is hidden | Must |
| 6 | App icon is correctly restored when Dock is re-enabled | Must (already fixed) |
| 7 | Config persists `showDockIcon` across restarts | Must (already fixed) |

---

## Architecture

### Data Flow

```
User toggles "Show dock icon"
  → Frontend calls api.platform.setDockVisible(false)
  → Rust: tray_manager.create() then setActivationPolicy_(Accessory)
  → Left-click tray → toggle_window()
  → Right-click tray → build_menu() → show context menu
  → Window close button → intercept CloseRequested → hide window
```

### Module Layout

```
src-tauri/src/
├── commands/
│   ├── tray.rs              # NEW: TrayIconManager struct + commands
│   ├── window.rs            # MODIFY: set_dock_visible delegates to TrayIconManager
│   └── mod.rs               # MODIFY: add tray module
├── lib.rs                   # MODIFY: startup tray init + close interception
└── icons/32x32.png          # EXISTING: tray icon source
```

No frontend changes required. All tray logic lives in Rust.

---

## TrayIconManager

**File:** `commands/tray.rs`

```rust
pub struct TrayIconManager {
    icon: Option<tauri::tray::TrayIcon>,
    app_handle: tauri::AppHandle,
}
```

### Methods

| Method | Description |
|--------|-------------|
| `new(app_handle)` | Create manager with app handle reference |
| `create()` | Build `TrayIcon` with icon, tooltip, event handlers, and context menu |
| `destroy()` | Remove tray icon if present |
| `set_visible(visible: bool)` | Orchestration entry: `visible=false` → create tray then hide dock; `visible=true` → show dock then destroy tray |
| `toggle_window()` | Show window if hidden (activate + show), hide if visible |
| `build_menu() -> Menu` | Build right-click context menu with window toggle, recent sessions submenu, quit |

### Tray Icon Configuration

- **Icon:** `icons/32x32.png` — macOS scales to 22x22 menu bar size automatically
- **Tooltip:** "claude-devtools"
- **Template image:** Set `icon_as_template(true)` so macOS adapts to light/dark menu bar

### Event Handlers

```rust
.on_tray_icon_event(|tray, event| {
    match event {
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } => { /* toggle window */ }
        TrayIconEvent::Click {
            button: MouseButton::Right,
            button_state: MouseButtonState::Up,
            ..
        } => { /* show context menu (default behavior) */ }
        _ => {}
    }
})
```

---

## Right-Click Context Menu

```
┌─────────────────────┐
│ 显示/隐藏窗口        │
│─────────────────────│
│ 最近会话            │  ← submenu, max 5 entries
│   ├── Session A     │
│   ├── Session B     │
│   └── ...           │
│─────────────────────│
│ 退出                │
└─────────────────────┘
```

Built with `muda::Menu` (already available as Tauri transitive dependency):

- **"显示/隐藏窗口"** — calls `toggle_window()`
- **"最近会话"** submenu — queries `AppState` for recent sessions, limited to 5. Each entry opens that session in the app window
- **"退出"** — calls `app.exit(0)`

**Refresh strategy:** Menu is rebuilt on every right-click (no polling). Recent sessions are queried from `AppState` at menu-build time, ensuring data is always current with zero overhead.

---

## Dock ↔ Tray Coordination

### `set_dock_visible` Command (modified)

```rust
pub async fn set_dock_visible(
    tray: tauri::State<'_, TrayIconManager>,
    state: tauri::State<'_, Arc<RwLock<AppState>>>,
    visible: bool,
) -> Result<(), String> {
    if visible {
        // 1. Restore dock
        setActivationPolicy_(Regular) + restore icon
        // 2. Remove tray (no longer needed)
        tray.destroy()
    } else {
        // 1. Create tray FIRST (ensures user always has an entry point)
        tray.create()
        // 2. Then hide dock
        setActivationPolicy_(Accessory)
    }
    // Persist config
    config_manager.update_config("general", { "showDockIcon": visible })
}
```

### Startup Flow (lib.rs setup)

```rust
// After config is loaded:
if !config.general.showDockIcon {
    tray_manager.create();              // Create tray first
    setActivationPolicy_(Accessory);    // Then hide dock
}
```

### Window Close Interception

```rust
.on_window_event(|window, event| {
    if let WindowEvent::CloseRequested { api, .. } = event {
        // When dock is hidden, close button hides to tray instead
        if !is_dock_visible() {
            window.hide().unwrap();
            api.prevent_close();
        }
    }
})
```

---

## State Management

`TrayIconManager` is registered as Tauri managed state via `app.manage()` in `lib.rs`. It shares the same lifetime as the app and is accessible from any command via `tauri::State<'_, TrayIconManager>`.

Access to `AppState` (for recent sessions) is through `app.state::<Arc<RwLock<AppState>>>()`.

---

## Testing

| Test | Type | Description |
|------|------|-------------|
| Tray creation | Unit | Verify `TrayIconManager::create()` builds icon and menu without error |
| Tray destruction | Unit | Verify `TrayIconManager::destroy()` removes icon |
| Window toggle | Unit | Mock window, verify show/hide cycle |
| Menu structure | Unit | Verify menu items: window toggle, recent sessions, quit |
| Close interception | Integration | Close window when dock hidden → window hides, app stays running |
| Close passthrough | Integration | Close window when dock visible → app exits normally |
| Config persistence | Integration | Restart app with `showDockIcon: false` → tray appears on startup |
