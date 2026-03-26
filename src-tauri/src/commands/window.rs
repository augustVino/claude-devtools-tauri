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
    use objc::*;

    unsafe {
        let app = NSApplication::sharedApplication(nil);
        if visible {
            // Restore dock icon by setting Regular policy
            app.setActivationPolicy_(
                NSApplicationActivationPolicy::NSApplicationActivationPolicyRegular,
            );
            // Restore the saved dock icon, then remove tray
            let mut tray_guard = tray.lock().map_err(|e| e.to_string())?;
            tray_guard.restore_dock_icon();
            tray_guard.destroy();
        } else {
            // Save dock icon BEFORE switching to Accessory (known macOS bug:
            // icon is lost when switching back to Regular)
            {
                let tray_guard = tray.lock().map_err(|e| e.to_string())?;
                tray_guard.save_dock_icon();
            }
            // Create tray FIRST (ensures user always has an entry point)
            tray.lock().map_err(|e| e.to_string())?.create()?;
            // Then hide dock
            app.setActivationPolicy_(
                NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
            );
            // Re-activate the app after switching to Accessory policy
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
