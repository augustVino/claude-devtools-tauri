use tauri::{command, State};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::commands::AppState;
use crate::types::config::AppConfig;
use tauri_plugin_opener::OpenerExt;

// =============================================================================
// 配置命令
// =============================================================================

/// 获取当前完整的应用配置。
#[command]
pub async fn get_config(
    state: State<'_, Arc<RwLock<AppState>>>,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.get_config())
}

/// 更新配置的指定分区。
///
/// `section` 为配置分区名称（如 "general"、"notification"），`data` 为该分区的 JSON 数据。
/// 采用深度合并策略，未变更的字段保留原值。
#[command]
pub async fn update_config(
    state: State<'_, Arc<RwLock<AppState>>>,
    section: String,
    data: serde_json::Value,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    app_state.config_manager.update_config(&section, data)
}

// =============================================================================
// 通知忽略正则
// =============================================================================

/// 添加通知忽略正则表达式。
///
/// 匹配该正则的错误消息将不会触发通知。
#[command]
pub async fn add_ignore_regex(
    state: State<'_, Arc<RwLock<AppState>>>,
    pattern: String,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    app_state.config_manager.add_ignore_regex(pattern)
}

/// 移除指定的通知忽略正则表达式。
#[command]
pub async fn remove_ignore_regex(
    state: State<'_, Arc<RwLock<AppState>>>,
    pattern: String,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.remove_ignore_regex(pattern))
}

// =============================================================================
// 会话置顶/隐藏
// =============================================================================

/// 置顶指定会话。
#[command]
pub async fn pin_session(
    state: State<'_, Arc<RwLock<AppState>>>,
    project_id: String,
    session_id: String,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.pin_session(project_id, session_id))
}

/// 取消置顶指定会话。
#[command]
pub async fn unpin_session(
    state: State<'_, Arc<RwLock<AppState>>>,
    project_id: String,
    session_id: String,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.unpin_session(project_id, session_id))
}

/// 隐藏指定会话。
#[command]
pub async fn hide_session(
    state: State<'_, Arc<RwLock<AppState>>>,
    project_id: String,
    session_id: String,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.hide_session(project_id, session_id))
}

/// 取消隐藏指定会话。
#[command]
pub async fn unhide_session(
    state: State<'_, Arc<RwLock<AppState>>>,
    project_id: String,
    session_id: String,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.unhide_session(project_id, session_id))
}

// =============================================================================
// 通知暂停
// =============================================================================

/// 暂停通知推送指定分钟数。
///
/// `minutes = -1` 表示"暂停到明天午夜"。
#[command]
pub async fn snooze(
    state: State<'_, Arc<RwLock<AppState>>>,
    minutes: i32,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    if minutes == -1 {
        Ok(app_state.config_manager.snooze_until_tomorrow())
    } else if minutes <= 0 {
        Err("Minutes must be a positive number".to_string())
    } else if minutes > 24 * 60 {
        Err("Minutes must be 1440 or less (24 hours)".to_string())
    } else {
        Ok(app_state.config_manager.snooze(minutes as u32))
    }
}

/// 清除通知暂停设置，恢复通知推送。
#[command]
pub async fn clear_snooze(
    state: State<'_, Arc<RwLock<AppState>>>,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.clear_snooze())
}

// =============================================================================
// 通知触发器
// =============================================================================

/// 添加自定义通知触发器。
#[command]
pub async fn add_trigger(
    state: State<'_, Arc<RwLock<AppState>>>,
    trigger: crate::types::config::NotificationTrigger,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    app_state.config_manager.add_trigger(trigger)
}

/// 更新指定通知触发器的配置。
#[command]
pub async fn update_trigger(
    state: State<'_, Arc<RwLock<AppState>>>,
    trigger_id: String,
    updates: serde_json::Value,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    app_state.config_manager.update_trigger(&trigger_id, updates)
}

/// 删除指定通知触发器。
#[command]
pub async fn remove_trigger(
    state: State<'_, Arc<RwLock<AppState>>>,
    trigger_id: String,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    app_state.config_manager.remove_trigger(&trigger_id)
}

/// 获取所有通知触发器列表。
#[command]
pub async fn get_triggers(
    state: State<'_, Arc<RwLock<AppState>>>,
) -> Result<Vec<crate::types::config::NotificationTrigger>, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.get_triggers())
}

/// 测试通知触发器。
///
/// 使用项目扫描器扫描现有会话数据，检验触发器是否能匹配到错误。
#[command]
pub async fn test_trigger(
    trigger: crate::types::config::NotificationTrigger,
) -> Result<crate::types::config::TriggerTestResult, String> {
    use crate::discovery::project_scanner::ProjectScanner;
    use crate::error::error_trigger_tester;

    let scanner = ProjectScanner::with_paths(
        crate::utils::get_projects_base_path(),
        crate::utils::get_todos_base_path(),
        std::sync::Arc::new(crate::infrastructure::fs_provider::LocalFsProvider::new()),
    );
    Ok(error_trigger_tester::test_trigger(&trigger, &scanner, None).await)
}

// =============================================================================
// 仓库忽略
// =============================================================================

/// 添加仓库到忽略列表。
///
/// 被忽略的仓库将不会出现在项目列表中。
#[command]
pub async fn add_ignore_repository(
    state: State<'_, Arc<RwLock<AppState>>>,
    repository_id: String,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.add_ignore_repository(repository_id))
}

/// 从忽略列表中移除指定仓库。
#[command]
pub async fn remove_ignore_repository(
    state: State<'_, Arc<RwLock<AppState>>>,
    repository_id: String,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.remove_ignore_repository(repository_id))
}

// =============================================================================
// 批量隐藏/取消隐藏会话
// =============================================================================

/// 批量隐藏指定会话。
#[command]
pub async fn hide_sessions(
    state: State<'_, Arc<RwLock<AppState>>>,
    project_id: String,
    session_ids: Vec<String>,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.hide_sessions(project_id, session_ids))
}

/// 批量取消隐藏指定会话。
#[command]
pub async fn unhide_sessions(
    state: State<'_, Arc<RwLock<AppState>>>,
    project_id: String,
    session_ids: Vec<String>,
) -> Result<AppConfig, String> {
    let app_state = state.read().await;
    Ok(app_state.config_manager.unhide_sessions(project_id, session_ids))
}

// =============================================================================
// 编辑器与 Claude 根目录
// =============================================================================

/// 在系统编辑器中打开配置文件。
///
/// 按优先级尝试 `$VISUAL`、`$EDITOR` 环境变量，以及常见编辑器（cursor、code、zed、subl），
/// 最终回退到系统默认打开方式。
#[command]
pub async fn open_in_editor(
    app: tauri::AppHandle,
    state: State<'_, Arc<RwLock<AppState>>>,
) -> Result<(), String> {
    let app_state = state.read().await;
    let config_path = app_state.config_manager.get_config_path();
    drop(app_state);

    let config_path_str = config_path.to_string_lossy().to_string();

    // 尝试 $VISUAL 环境变量
    if let Ok(editor) = std::env::var("VISUAL") {
        if try_spawn_editor(&editor, &config_path_str).await {
            return Ok(());
        }
    }

    // 尝试 $EDITOR 环境变量
    if let Ok(editor) = std::env::var("EDITOR") {
        if try_spawn_editor(&editor, &config_path_str).await {
            return Ok(());
        }
    }

    // 尝试常见编辑器
    for editor in &["cursor", "code", "zed", "subl"] {
        if try_spawn_editor(editor, &config_path_str).await {
            return Ok(());
        }
    }

    // 回退到系统默认打开方式
    app.opener()
        .open_path(&config_path_str, None::<&str>)
        .map_err(|e| format!("Failed to open config file: {}", e))
}

/// 获取 Claude 根目录信息。
///
/// 返回默认路径（`~/.claude`）、用户自定义路径和实际使用的解析路径。
#[command]
pub async fn get_claude_root_info(
    state: State<'_, Arc<RwLock<AppState>>>,
) -> Result<crate::types::config::ClaudeRootInfo, String> {
    let app_state = state.read().await;
    let config = app_state.config_manager.get_config();
    drop(app_state);

    let custom_path = config.general.claude_root_path.clone();
    let default_path = dirs::home_dir()
        .map(|h| h.join(".claude").to_string_lossy().to_string())
        .unwrap_or_default();
    let resolved_path = custom_path.clone().unwrap_or_else(|| default_path.clone());

    Ok(crate::types::config::ClaudeRootInfo {
        default_path,
        resolved_path,
        custom_path,
    })
}

// =============================================================================
// 项目目录检查
// =============================================================================

/// 检查指定路径下是否存在 `projects` 目录。
#[command]
pub async fn check_projects_dir_exists(path: String) -> Result<bool, String> {
    Ok(std::path::Path::new(&path).join("projects").is_dir())
}

/// 尝试启动编辑器进程打开文件。成功返回 `true`。
async fn try_spawn_editor(editor: &str, file_path: &str) -> bool {
    tokio::process::Command::new(editor)
        .arg(file_path)
        .spawn()
        .map(|_| true)
        .unwrap_or(false)
}
