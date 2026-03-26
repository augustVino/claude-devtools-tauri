use std::sync::Arc;
use tauri::{command, AppHandle, Manager};
use tokio::sync::RwLock;

use super::{sessions::AppState, tray::TrayIconManager};

#[command]
pub async fn minimize(app: AppHandle) -> Result<(), String> {
    app.get_webview_window("main")
        .map(|w| w.minimize().map_err(|e| e.to_string()))
        .unwrap_or(Err("No main window".into()))
}

#[command]
pub async fn maximize(app: AppHandle) -> Result<bool, String> {
    let window = app
        .get_webview_window("main")
        .ok_or("No main window")?;

    if window.is_maximized().unwrap_or(false) {
        window.unmaximize().map_err(|e| e.to_string())?;
    } else {
        window.maximize().map_err(|e| e.to_string())?;
    }

    window.is_maximized().map_err(|e| e.to_string())
}

#[command]
pub async fn close(app: AppHandle) -> Result<(), String> {
    app.get_webview_window("main")
        .map(|w| w.close().map_err(|e| e.to_string()))
        .unwrap_or(Err("No main window".into()))
}

#[command]
pub async fn is_maximized(app: AppHandle) -> Result<bool, String> {
    app.get_webview_window("main")
        .map(|w| w.is_maximized().map_err(|e| e.to_string()))
        .unwrap_or(Ok(false))
}

#[command]
pub async fn relaunch(app: tauri::AppHandle) -> Result<(), String> {
    app.request_restart();
    Ok(())
}

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
    use objc::runtime::Object;
    use objc::*;

    unsafe {
        let app = NSApplication::sharedApplication(nil);
        if visible {
            // Restore dock icon by setting Regular policy
            app.setActivationPolicy_(
                NSApplicationActivationPolicy::NSApplicationActivationPolicyRegular,
            );
            // Explicitly restore app icon (known macOS bug: icon not restored after Accessory -> Regular)
            let icon_name = NSString::alloc(nil).init_str("AppIcon");
            let app_icon: *mut Object = msg_send![class!(NSImage), imageNamed: icon_name];
            if !app_icon.is_null() {
                let _: () = msg_send![app, setApplicationIconImage: app_icon];
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
            // Re-activate the app after switching to Accessory policy
            // macOS deactivates the app when switching to Accessory mode,
            // causing the window to lose focus and the first menu click to fail
            let _: () = msg_send![app, activateIgnoringOtherApps: true];
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

#[cfg(not(target_os = "macos"))]
#[command]
pub async fn set_dock_visible(_visible: bool) -> Result<(), String> {
    Err("Dock hiding is only supported on macOS".to_string())
}
