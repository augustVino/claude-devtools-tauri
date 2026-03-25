use tauri::{command, AppHandle, Manager};

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
