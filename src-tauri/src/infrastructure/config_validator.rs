//! 配置校验模块 — 提供无状态的纯校验函数和 JSON 合并工具函数。
//!
//! 从 config_manager.rs 中提取，包含：
//! - JSON 深度合并 (json_merge)
//! - 配置分区 payload 校验 (validate_update_payload 及各分区子校验器)
//! - 分区更新辅助 (update_section)

use serde_json;

/// 递归深度合并两个 JSON Value。
/// 对于 Object 类型递归合并；非 Object 类型直接用 patch 覆盖 base。
pub fn json_merge(base: &serde_json::Value, patch: &serde_json::Value) -> serde_json::Value {
    match (base, patch) {
        (serde_json::Value::Object(base_map), serde_json::Value::Object(patch_map)) => {
            let mut merged = base_map.clone();
            for (key, value) in patch_map {
                let entry = merged.remove(key).unwrap_or(serde_json::Value::Null);
                merged.insert(key.clone(), json_merge(&entry, value));
            }
            serde_json::Value::Object(merged)
        }
        (_, patch) => patch.clone(),
    }
}

/// 更新 JSON 中指定分区的值（深度合并）
pub fn update_section(current: &serde_json::Value, section: &str, data: &serde_json::Value) -> serde_json::Value {
    let mut updated = current.clone();
    if let Some(current_section) = updated.get_mut(section) {
        *current_section = json_merge(current_section, data);
    } else {
        updated.as_object_mut().map(|map| map.insert(section.to_string(), data.clone()));
    }
    updated
}

/// 校验 `config:update` 的 payload，与 Electron 端 `validateConfigUpdatePayload` 对齐。
///
/// 返回 `Ok(())` 表示校验通过，`Err(message)` 表示校验失败。
pub fn validate_update_payload(section: &str, data: &serde_json::Value) -> Result<(), String> {
    let obj = match data {
        serde_json::Value::Object(map) if !map.is_empty() => map,
        serde_json::Value::Object(_) => return Ok(()), // 空对象，无字段需要校验
        _ => return Err(format!("{section} update must be an object")),
    };

    match section {
        "notifications" => validate_notifications_payload(obj),
        "general" => validate_general_payload(obj),
        "display" => validate_display_payload(obj),
        "httpServer" => validate_http_server_payload(obj),
        "ssh" => validate_ssh_payload(obj),
        "sessions" => Ok(()), // sessions 分区由内部逻辑管理，不对外暴露更新
        _ => Ok(()), // 其他 section 已在 update_config 白名单中拦截
    }
}

/// 校验 notifications 分区的 payload
fn validate_notifications_payload(data: &serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
    let allowed_keys = [
        "enabled",
        "soundEnabled",
        "includeSubagentErrors",
        "ignoredRegex",
        "ignoredRepositories",
        "snoozedUntil",
        "snoozeMinutes",
        "triggers",
    ];

    for key in data.keys() {
        if !allowed_keys.contains(&key.as_str()) {
            return Err(format!("notifications.{key} is not supported via config:update"));
        }
    }

    // enabled, soundEnabled, includeSubagentErrors: must be boolean
    for bool_key in &["enabled", "soundEnabled", "includeSubagentErrors"] {
        if let Some(v) = data.get(*bool_key) {
            if !v.is_boolean() {
                return Err(format!("notifications.{bool_key} must be a boolean"));
            }
        }
    }

    // ignoredRegex: must be string[]
    if let Some(v) = data.get("ignoredRegex") {
        if !v.is_array() || !v.as_array().unwrap().iter().all(|item| item.is_string()) {
            return Err("notifications.ignoredRegex must be a string[]".to_string());
        }
    }

    // ignoredRepositories: must be string[]
    if let Some(v) = data.get("ignoredRepositories") {
        if !v.is_array() || !v.as_array().unwrap().iter().all(|item| item.is_string()) {
            return Err("notifications.ignoredRepositories must be a string[]".to_string());
        }
    }

    // snoozedUntil: must be number >= 0 or null
    if let Some(v) = data.get("snoozedUntil") {
        match v {
            serde_json::Value::Null => {}
            serde_json::Value::Number(n) => {
                if n.as_f64().is_none_or(|f| f.is_nan() || f.is_infinite() || f < 0.0) {
                    return Err("notifications.snoozedUntil must be a non-negative number or null".to_string());
                }
            }
            _ => return Err("notifications.snoozedUntil must be a non-negative number or null".to_string()),
        }
    }

    // snoozeMinutes: must be integer 1-1440
    if let Some(v) = data.get("snoozeMinutes") {
        match v.as_i64() {
            Some(n) => {
                if n < 1 || n > 1440 {
                    return Err("notifications.snoozeMinutes must be between 1 and 1440".to_string());
                }
            }
            None => return Err("notifications.snoozeMinutes must be an integer".to_string()),
        }
    }

    // triggers: must be valid trigger array
    if let Some(v) = data.get("triggers") {
        let arr = match v.as_array() {
            Some(a) => a,
            None => return Err("notifications.triggers must be an array".to_string()),
        };
        for (i, trigger) in arr.iter().enumerate() {
            validate_trigger_payload(trigger).map_err(|e| format!("notifications.triggers[{i}]: {e}"))?;
        }
    }

    Ok(())
}

/// 校验单个 trigger 对象（与 Electron 端 isValidTrigger 对齐）
fn validate_trigger_payload(trigger: &serde_json::Value) -> Result<(), String> {
    let obj = trigger
        .as_object()
        .ok_or("trigger must be an object")?;

    // id: non-empty string
    match obj.get("id").and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => {}
        _ => return Err("trigger.id must be a non-empty string".to_string()),
    }

    // name: non-empty string
    match obj.get("name").and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => {}
        _ => return Err("trigger.name must be a non-empty string".to_string()),
    }

    // enabled: boolean (required)
    match obj.get("enabled").and_then(|v| v.as_bool()) {
        Some(_) => {}
        None => return Err("trigger.enabled must be a boolean".to_string()),
    }

    // contentType: must be one of valid values (required)
    let valid_content_types = ["tool_result", "tool_use", "thinking", "text"];
    match obj.get("contentType").and_then(|v| v.as_str()) {
        Some(s) if valid_content_types.contains(&s) => {}
        _ => return Err("trigger.contentType must be one of: tool_result, tool_use, thinking, text".to_string()),
    }

    // mode: must be one of valid values (required)
    let valid_modes = ["error_status", "content_match", "token_threshold"];
    match obj.get("mode").and_then(|v| v.as_str()) {
        Some(s) if valid_modes.contains(&s) => {}
        _ => return Err("trigger.mode must be one of: error_status, content_match, token_threshold".to_string()),
    }

    Ok(())
}

/// 校验 general 分区的 payload
fn validate_general_payload(data: &serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
    let allowed_keys = [
        "launchAtLogin",
        "showDockIcon",
        "theme",
        "defaultTab",
        "claudeRootPath",
        "autoExpandAIGroups",
        "useNativeTitleBar",
    ];

    for key in data.keys() {
        if !allowed_keys.contains(&key.as_str()) {
            return Err(format!("general.{key} is not a valid setting"));
        }
    }

    // launchAtLogin, showDockIcon, autoExpandAIGroups, useNativeTitleBar: must be boolean
    for bool_key in &["launchAtLogin", "showDockIcon", "autoExpandAIGroups", "useNativeTitleBar"] {
        if let Some(v) = data.get(*bool_key) {
            if !v.is_boolean() {
                return Err(format!("general.{bool_key} must be a boolean"));
            }
        }
    }

    // theme: enum
    if let Some(v) = data.get("theme") {
        let valid = ["dark", "light", "system"];
        match v.as_str() {
            Some(s) if valid.contains(&s) => {}
            _ => return Err("general.theme must be one of: dark, light, system".to_string()),
        }
    }

    // defaultTab: enum
    if let Some(v) = data.get("defaultTab") {
        let valid = ["dashboard", "last-session"];
        match v.as_str() {
            Some(s) if valid.contains(&s) => {}
            _ => return Err("general.defaultTab must be one of: dashboard, last-session".to_string()),
        }
    }

    // claudeRootPath: must be absolute path or null
    if let Some(v) = data.get("claudeRootPath") {
        match v {
            serde_json::Value::Null => {}
            serde_json::Value::String(s) if s.trim().is_empty() => {}
            serde_json::Value::String(s) => {
                if !std::path::Path::new(s.trim()).is_absolute() {
                    return Err("general.claudeRootPath must be an absolute path".to_string());
                }
            }
            _ => return Err("general.claudeRootPath must be an absolute path string or null".to_string()),
        }
    }

    Ok(())
}

/// 校验 display 分区的 payload
fn validate_display_payload(data: &serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
    let allowed_keys = ["showTimestamps", "compactMode", "syntaxHighlighting"];

    for key in data.keys() {
        if !allowed_keys.contains(&key.as_str()) {
            return Err(format!("display.{key} is not a valid setting"));
        }
    }

    // All fields must be boolean
    for bool_key in &allowed_keys {
        if let Some(v) = data.get(*bool_key) {
            if !v.is_boolean() {
                return Err(format!("display.{bool_key} must be a boolean"));
            }
        }
    }

    Ok(())
}

/// 校验 httpServer 分区的 payload
fn validate_http_server_payload(data: &serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
    let allowed_keys = ["enabled", "port"];

    for key in data.keys() {
        if !allowed_keys.contains(&key.as_str()) {
            return Err(format!("httpServer.{key} is not a valid setting"));
        }
    }

    // enabled: must be boolean
    if let Some(v) = data.get("enabled") {
        if !v.is_boolean() {
            return Err("httpServer.enabled must be a boolean".to_string());
        }
    }

    // port: must be integer 1024-65535
    if let Some(v) = data.get("port") {
        match v.as_i64() {
            Some(n) => {
                if n < 1024 || n > 65535 {
                    return Err("httpServer.port must be an integer between 1024 and 65535".to_string());
                }
            }
            None => return Err("httpServer.port must be an integer".to_string()),
        }
    }

    Ok(())
}

/// 校验 ssh 分区的 payload
fn validate_ssh_payload(data: &serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
    let allowed_keys = ["autoReconnect", "lastConnection", "profiles", "lastActiveContextId"];

    for key in data.keys() {
        if !allowed_keys.contains(&key.as_str()) {
            return Err(format!("ssh.{key} is not a valid setting"));
        }
    }

    // autoReconnect: must be boolean
    if let Some(v) = data.get("autoReconnect") {
        if !v.is_boolean() {
            return Err("ssh.autoReconnect must be a boolean".to_string());
        }
    }

    // lastActiveContextId: must be string
    if let Some(v) = data.get("lastActiveContextId") {
        if !v.is_string() {
            return Err("ssh.lastActiveContextId must be a string".to_string());
        }
    }

    // lastConnection: must be object or null
    if let Some(v) = data.get("lastConnection") {
        match v {
            serde_json::Value::Null => {}
            serde_json::Value::Object(_) => {}
            _ => return Err("ssh.lastConnection must be an object or null".to_string()),
        }
    }

    // profiles: must be valid profile array
    if let Some(v) = data.get("profiles") {
        let arr = match v.as_array() {
            Some(a) => a,
            None => return Err("ssh.profiles must be an array".to_string()),
        };
        for (i, profile) in arr.iter().enumerate() {
            validate_ssh_profile_payload(profile).map_err(|e| format!("ssh.profiles[{i}]: {e}"))?;
        }
    }

    Ok(())
}

/// 校验单个 SSH profile 对象
fn validate_ssh_profile_payload(profile: &serde_json::Value) -> Result<(), String> {
    let obj = profile
        .as_object()
        .ok_or("SSH profile must be an object")?;

    // id: non-empty string (required)
    match obj.get("id").and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => {}
        _ => return Err("id must be a non-empty string".to_string()),
    }

    // name: must be string (required)
    match obj.get("name") {
        Some(v) if v.is_string() => {}
        _ => return Err("name must be a string".to_string()),
    }

    // host: must be string (required)
    match obj.get("host") {
        Some(v) if v.is_string() => {}
        _ => return Err("host must be a string".to_string()),
    }

    // port: must be number (required)
    match obj.get("port") {
        Some(v) if v.is_number() => {}
        _ => return Err("port must be a number".to_string()),
    }

    // username: must be string (required)
    match obj.get("username") {
        Some(v) if v.is_string() => {}
        _ => return Err("username must be a string".to_string()),
    }

    // authMethod: must be one of valid values (required)
    let valid_methods = ["password", "privateKey", "agent", "auto"];
    match obj.get("authMethod").and_then(|v| v.as_str()) {
        Some(s) if valid_methods.contains(&s) => {}
        _ => return Err("authMethod must be one of: password, privateKey, agent, auto".to_string()),
    }

    Ok(())
}
