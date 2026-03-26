use std::sync::Arc;
use tauri::{command, AppHandle, Manager};
use tokio::sync::RwLock;

use super::sessions::AppState;

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
            // macOS does not restore the app icon when switching back from Accessory.
            // Re-set it via NSBundle + NSImage using objc messaging.
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
        } else {
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

#[cfg(not(target_os = "macos"))]
#[command]
pub async fn set_dock_visible(_visible: bool) -> Result<(), String> {
    Err("Dock hiding is only supported on macOS".to_string())
}
