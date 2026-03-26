// 防止 Windows Release 模式下弹出额外的控制台窗口，请勿移除!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// 应用程序入口点。
/// 委托给 `app_lib::run()` 执行 Tauri 应用初始化。
fn main() {
  app_lib::run();
}
