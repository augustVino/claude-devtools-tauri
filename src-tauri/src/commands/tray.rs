//! TrayIconManager - Manages the system tray icon lifecycle and context menu.
//!
//! Responsibilities:
//! - Create/destroy tray icon based on dock visibility preference
//! - Handle left-click to toggle window visibility
//! - Provide context menu with recent sessions and quit option
//! - Coordinate with set_dock_visible command

use tauri::{
    AppHandle, Emitter, Manager,
    menu::{Menu, MenuItem, PredefinedMenuItem, SubmenuBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

use crate::discovery::ProjectScanner;

/// Manages the system tray icon and its context menu.
pub struct TrayIconManager {
    icon: Option<tauri::tray::TrayIcon>,
    app_handle: AppHandle,
    dock_visible: bool,
}

impl TrayIconManager {
    /// Create a new TrayIconManager.
    pub fn new(app_handle: AppHandle) -> Self {
        Self {
            icon: None,
            app_handle,
            dock_visible: true,
        }
    }

    /// Check if the dock icon is currently visible.
    pub fn is_dock_visible(&self) -> bool {
        self.dock_visible
    }

    /// Create tray icon with context menu.
    /// Must be called on the main thread (macOS UI requirement).
    pub fn create(&mut self) -> Result<(), String> {
        if self.icon.is_some() {
            return Ok(()); // Already created
        }

        // Build recent sessions submenu
        let scanner = ProjectScanner::new();
        let projects = scanner.scan();

        let recent: Vec<_> = projects
            .into_iter()
            .filter(|p| p.most_recent_session.is_some())
            .take(5)
            .collect();

        let mut submenu_builder = SubmenuBuilder::new(&self.app_handle, "Recent Sessions");

        if recent.is_empty() {
            submenu_builder = submenu_builder.item(
                &MenuItem::with_id(&self.app_handle, "empty", "No Recent Sessions", false, None::<&str>)
                    .map_err(|e| format!("Failed to create empty menu item: {e}"))?,
            );
        } else {
            for project in &recent {
                let label = &project.name;
                // Use most_recent_session for the session ID (this is what we filtered on)
                let session_id = project.most_recent_session.clone().unwrap_or_default();
                let item_id = format!("session:{}:{}", project.id, session_id);

                submenu_builder = submenu_builder.item(
                    &MenuItem::with_id(&self.app_handle, &item_id, label, true, None::<&str>)
                        .map_err(|e| format!("Failed to create session menu item: {e}"))?,
                );
            }
        }

        let recent_submenu = submenu_builder
            .build()
            .map_err(|e| format!("Failed to build recent sessions submenu: {e}"))?;

        // Build the main menu
        let menu = Menu::with_items(
            &self.app_handle,
            &[
                &MenuItem::with_id(&self.app_handle, "toggle", "Show/Hide Window", true, None::<&str>)
                    .map_err(|e| format!("Failed to create toggle menu item: {e}"))?,
                &PredefinedMenuItem::separator(&self.app_handle)
                    .map_err(|e| format!("Failed to create separator: {e}"))?,
                &recent_submenu,
                &PredefinedMenuItem::separator(&self.app_handle)
                    .map_err(|e| format!("Failed to create separator: {e}"))?,
                &MenuItem::with_id(&self.app_handle, "quit", "Quit", true, None::<&str>)
                    .map_err(|e| format!("Failed to create quit menu item: {e}"))?,
            ],
        )
        .map_err(|e| format!("Failed to create tray menu: {e}"))?;

        // Build the tray icon
        let tray = TrayIconBuilder::new()
            .icon(
                self.app_handle
                    .default_window_icon()
                    .ok_or_else(|| "No default window icon configured".to_string())?
                    .clone(),
            )
            .tooltip("claude-devtools")
            .icon_as_template(true)
            .menu(&menu)
            .on_menu_event(|app, event| {
                match event.id().as_ref() {
                    "toggle" => {
                        Self::toggle_window(app);
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    id if id.starts_with("session:") => {
                        // Parse session:project_id:session_id format
                        let parts: Vec<&str> = id.splitn(3, ':').collect();
                        if parts.len() == 3 {
                            let project_id = parts[1].to_string();
                            let session_id = parts[2].to_string();

                            // Emit event for frontend to navigate
                            let payload = OpenSessionPayload {
                                project_id: project_id.clone(),
                                session_id: session_id.clone(),
                            };
                            let _ = app.emit("tray:open-session", &payload);

                            // Show and focus the window
                            Self::toggle_window(app);
                        }
                    }
                    _ => {}
                }
            })
            .on_tray_icon_event(|tray, event| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    Self::toggle_window(tray.app_handle());
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
                            use cocoa::appkit::NSApplication;
                            let ns_app = cocoa::appkit::NSApplication::sharedApplication(cocoa::base::nil);
                            let _: () = cocoa::appkit::NSApplication::activateIgnoringOtherApps_(ns_app, true);
                        }
                    });
                }
            }
        }
    }

    /// Rebuild the tray icon with updated menu (call from file watcher when sessions change).
    /// Since Tauri v2 TrayIcon does not expose set_menu(), we destroy and recreate.
    pub fn refresh_menu(&mut self) -> Result<(), String> {
        let was_dock_visible = self.dock_visible;
        self.destroy();
        self.create()?;
        self.dock_visible = was_dock_visible;
        Ok(())
    }
}

/// Payload for tray:open-session event.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenSessionPayload {
    project_id: String,
    session_id: String,
}