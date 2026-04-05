//! 会话操作：置顶、隐藏、批量操作。

use std::collections::HashMap;

use crate::error::AppError;
use crate::types::{HiddenSession, PinnedSession};

impl super::ConfigManager {
    // ===== 公共方法 =====

    pub async fn pin_session(&self, project_id: String, session_id: String) -> Result<crate::types::AppConfig, crate::error::AppError> {
        self.with_config_mut(|config| {
            let pins = config.sessions.pinned_sessions.entry(project_id.clone()).or_default();
            if !pins.iter().any(|p| p.session_id == session_id) {
                pins.insert(0, PinnedSession { session_id: session_id.clone(), pinned_at: super::defaults::now_millis() });
                true
            } else { false }
        }).await
    }

    pub async fn unpin_session(&self, project_id: String, session_id: String) -> Result<crate::types::AppConfig, crate::error::AppError> {
        self.with_config_mut(|config| {
            if let Some(pins) = config.sessions.pinned_sessions.get_mut(&project_id) {
                pins.retain(|p| p.session_id != session_id);
            }
            cleanup_empty_project(&mut config.sessions.pinned_sessions, &project_id);
            true
        }).await
    }

    pub async fn hide_session(&self, project_id: String, session_id: String) -> Result<crate::types::AppConfig, crate::error::AppError> {
        self.with_config_mut(|config| {
            let hidden = config.sessions.hidden_sessions.entry(project_id.clone()).or_default();
            if !hidden.iter().any(|h| h.session_id == session_id) {
                hidden.insert(0, HiddenSession { session_id: session_id.clone(), hidden_at: super::defaults::now_millis() });
                true
            } else { false }
        }).await
    }

    pub async fn unhide_session(&self, project_id: String, session_id: String) -> Result<crate::types::AppConfig, crate::error::AppError> {
        self.with_config_mut(|config| {
            if let Some(hidden) = config.sessions.hidden_sessions.get_mut(&project_id) {
                hidden.retain(|h| h.session_id != session_id);
            }
            cleanup_empty_project(&mut config.sessions.hidden_sessions, &project_id);
            true
        }).await
    }

    pub async fn hide_sessions(&self, project_id: String, session_ids: Vec<String>) -> Result<crate::types::AppConfig, crate::error::AppError> {
        if session_ids.is_empty() { return Ok(self.get_config().await); }
        let ts = super::defaults::now_millis();
        self.with_config_mut(|config| {
            let hidden = config.sessions.hidden_sessions.entry(project_id.clone()).or_default();
            let existing: std::collections::HashSet<String> = hidden.iter().map(|h| h.session_id.clone()).collect();
            let new_entries: Vec<HiddenSession> = session_ids.iter()
                .filter(|id| !existing.contains(*id))
                .map(|id| HiddenSession { session_id: id.clone(), hidden_at: ts })
                .collect();
            if !new_entries.is_empty() {
                let mut updated = new_entries; updated.extend(hidden.drain(..)); *hidden = updated;
                true
            } else { false }
        }).await
    }

    pub async fn unhide_sessions(&self, project_id: String, session_ids: Vec<String>) -> Result<crate::types::AppConfig, crate::error::AppError> {
        if session_ids.is_empty() { return Ok(self.get_config().await); }
        let to_remove: std::collections::HashSet<String> = session_ids.into_iter().collect();
        self.with_config_mut(|config| {
            if let Some(hidden) = config.sessions.hidden_sessions.get_mut(&project_id) {
                hidden.retain(|h| !to_remove.contains(&h.session_id));
            }
            cleanup_empty_project(&mut config.sessions.hidden_sessions, &project_id);
            true
        }).await
    }

    // ===== 内部骨架 =====

    /// 通用的"获取写锁→修改→释放锁→条件持久化→返回最新配置"骨架。
    pub(super) async fn with_config_mut<F>(&self, mutator: F) -> Result<crate::types::AppConfig, crate::error::AppError>
    where
        F: FnOnce(&mut crate::types::AppConfig) -> bool,
    {
        let mut config = self.config.write().await;
        let changed = mutator(&mut config);
        drop(config);
        if changed {
            self.persist_inner().await?;
        }
        Ok(self.get_config().await)
    }
}

/// 清理项目下空的会话列表条目
fn cleanup_empty_project<T>(sessions: &mut HashMap<String, Vec<T>>, project_id: &str) {
    if sessions.get(project_id).is_some_and(|v| v.is_empty()) { sessions.remove(project_id); }
}
