//! 上下文管理器 — 管理多个 ServiceContext 实例的注册、切换和销毁。

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::infrastructure::service_context::{ContextType, ServiceContext, ServiceContextConfig};

/// 上下文元数据。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ContextInfo {
    pub id: String,
    #[serde(rename = "type")]
    pub context_type: String,
}

impl ContextInfo {
    pub fn from_context(ctx: &ServiceContext) -> Self {
        Self {
            id: ctx.id.clone(),
            context_type: match ctx.context_type {
                ContextType::Local => "local".to_string(),
                ContextType::Ssh => "ssh".to_string(),
            },
        }
    }
}

/// 上下文切换结果。
#[derive(Debug)]
pub struct SwitchResult {
    pub previous_id: String,
    pub current_id: String,
}

/// 上下文管理器。
pub struct ContextManager {
    contexts: HashMap<String, Arc<RwLock<ServiceContext>>>,
    active_id: String,
}

impl ContextManager {
    pub fn new() -> Self {
        Self {
            contexts: HashMap::new(),
            active_id: "local".to_string(),
        }
    }

    pub fn register_context(&mut self, context: ServiceContext) -> Result<(), String> {
        let id = context.id.clone();
        if self.contexts.contains_key(&id) {
            return Err(format!("Context '{}' already registered", id));
        }
        self.contexts.insert(id, Arc::new(RwLock::new(context)));
        Ok(())
    }

    pub async fn replace_context(&mut self, context_id: &str, replacement: ServiceContext) -> Result<(), String> {
        if replacement.id != context_id {
            return Err(format!("Replacement ID '{}' does not match '{}'", replacement.id, context_id));
        }
        if !self.contexts.contains_key(context_id) {
            return Err(format!("Context '{}' not found", context_id));
        }
        if let Some(old) = self.contexts.get(context_id) {
            old.read().await.watcher_cancel_token.cancel();
        }
        let id = replacement.id.clone();
        self.contexts.insert(id, Arc::new(RwLock::new(replacement)));
        Ok(())
    }

    pub fn switch(&mut self, target_id: &str) -> Result<SwitchResult, String> {
        if target_id == self.active_id {
            return Err(format!("Already on context '{}'", target_id));
        }
        if !self.contexts.contains_key(target_id) {
            return Err(format!("Context '{}' not found", target_id));
        }
        let previous_id = std::mem::replace(&mut self.active_id, target_id.to_string());
        Ok(SwitchResult { previous_id, current_id: target_id.to_string() })
    }

    pub async fn destroy_context(&mut self, context_id: &str) -> Result<(), String> {
        if context_id == "local" {
            return Err("Cannot destroy the local context".to_string());
        }
        let context = self.contexts.remove(context_id)
            .ok_or_else(|| format!("Context '{}' not found", context_id))?;
        context.read().await.watcher_cancel_token.cancel();
        if self.active_id == context_id {
            self.active_id = "local".to_string();
        }
        Ok(())
    }

    pub fn get_active(&self) -> Option<Arc<RwLock<ServiceContext>>> {
        self.contexts.get(&self.active_id).cloned()
    }

    pub fn get(&self, context_id: &str) -> Option<Arc<RwLock<ServiceContext>>> {
        self.contexts.get(context_id).cloned()
    }

    pub fn has(&self, context_id: &str) -> bool {
        self.contexts.contains_key(context_id)
    }

    pub fn list(&self) -> Vec<ContextInfo> {
        let mut infos: Vec<ContextInfo> = self.contexts.values()
            .filter_map(|ctx| ctx.try_read().ok())
            .map(|guard| ContextInfo::from_context(&*guard))
            .collect();
        infos.sort_by(|a, b| {
            let a_active = a.id == self.active_id;
            let b_active = b.id == self.active_id;
            b_active.cmp(&a_active)
        });
        infos
    }

    pub fn get_active_id(&self) -> &str {
        &self.active_id
    }

    pub async fn dispose(&mut self) {
        for ctx in self.contexts.values() {
            if let Ok(guard) = ctx.try_read() {
                guard.watcher_cancel_token.cancel();
            }
        }
        self.contexts.clear();
    }
}

impl Default for ContextManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_config(id: &str, context_type: ContextType) -> ServiceContextConfig {
        ServiceContextConfig {
            id: id.to_string(),
            context_type,
            projects_dir: PathBuf::from("/tmp/test-projects"),
            todos_dir: PathBuf::from("/tmp/test-todos"),
        }
    }

    #[tokio::test]
    async fn test_register_and_get() {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config("local", ContextType::Local))).unwrap();
        assert!(mgr.has("local"));
        assert!(mgr.get("local").is_some());
    }

    #[tokio::test]
    async fn test_duplicate_registration_fails() {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config("local", ContextType::Local))).unwrap();
        let result = mgr.register_context(ServiceContext::new(make_config("local", ContextType::Local)));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already registered"));
    }

    #[tokio::test]
    async fn test_switch_context() {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config("local", ContextType::Local))).unwrap();
        mgr.register_context(ServiceContext::new(make_config("ssh-test", ContextType::Ssh))).unwrap();
        let result = mgr.switch("ssh-test").unwrap();
        assert_eq!(result.previous_id, "local");
        assert_eq!(result.current_id, "ssh-test");
        assert_eq!(mgr.get_active_id(), "ssh-test");
    }

    #[tokio::test]
    async fn test_switch_to_nonexistent_fails() {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config("local", ContextType::Local))).unwrap();
        let result = mgr.switch("nonexistent");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[tokio::test]
    async fn test_switch_to_same_is_error() {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config("local", ContextType::Local))).unwrap();
        let result = mgr.switch("local");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Already on context"));
    }

    #[tokio::test]
    async fn test_destroy_non_local() {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config("local", ContextType::Local))).unwrap();
        mgr.register_context(ServiceContext::new(make_config("ssh-test", ContextType::Ssh))).unwrap();
        mgr.switch("ssh-test").unwrap();
        mgr.destroy_context("ssh-test").await.unwrap();
        assert!(!mgr.has("ssh-test"));
        assert_eq!(mgr.get_active_id(), "local");
    }

    #[tokio::test]
    async fn test_destroy_local_fails() {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config("local", ContextType::Local))).unwrap();
        let result = mgr.destroy_context("local").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Cannot destroy"));
    }

    #[tokio::test]
    async fn test_list_returns_context_infos() {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config("local", ContextType::Local))).unwrap();
        mgr.register_context(ServiceContext::new(make_config("ssh-test", ContextType::Ssh))).unwrap();
        let list = mgr.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "local");
        assert_eq!(list[0].context_type, "local");
        assert_eq!(list[1].id, "ssh-test");
        assert_eq!(list[1].context_type, "ssh");
    }

    #[tokio::test]
    async fn test_replace_context() {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config("local", ContextType::Local))).unwrap();
        let replacement = ServiceContext::new(make_config("local", ContextType::Local));
        mgr.replace_context("local", replacement).await.unwrap();
        assert!(mgr.has("local"));
    }

    #[tokio::test]
    async fn test_replace_mismatched_id_fails() {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config("local", ContextType::Local))).unwrap();
        let replacement = ServiceContext::new(make_config("wrong-id", ContextType::Local));
        let result = mgr.replace_context("local", replacement).await;
        assert!(result.is_err());
    }
}
