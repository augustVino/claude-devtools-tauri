//! 上下文管理器 — 管理多个 ServiceContext 实例的注册、切换和销毁。

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::error::AppError;

#[allow(unused_imports)]
use crate::infrastructure::service_context::{ContextType, ServiceContext, ServiceContextConfig};

/// 上下文元数据。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ContextInfo {
    pub id: String,
    #[serde(rename = "type")]
    pub context_type: String,
}

/// 上下文切换返回值（与前端类型 `{ contextId: string }` 对齐）。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchResponse {
    pub context_id: String,
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

/// Watcher 生命周期操作指令。
///
/// 由 `switch_with_watcher_actions` 返回，调用方在持有写锁的异步上下文中
/// 根据指令执行 async watcher stop/start 操作。
///
/// 设计原因：`switch()` 是 sync 方法（只改 active_id 字符串），
/// 而 `stop_watcher_tasks()` / `spawn_watcher_tasks()` 是 async 方法，
/// 不能混在同一 `&mut self` 方法体内（RwLockWriteGuard 不允许跨 .await 存活）。
///
/// **关于 `should_stop_old` / `should_start_new`:** 当前两者始终同值（均由
/// `previous_id != current_id` 导出）。保留为独立字段是为未来场景预留扩展能力：
/// 例如 context 替换（replace_context）可能需要 "stop 旧 → 不启动新" 的语义。
#[derive(Debug, Clone)]
pub struct WatcherLifecycleActions {
    /// 是否需要停止旧 context 的 watcher
    pub should_stop_old: bool,
    /// 旧 context ID
    pub old_context_id: String,
    /// 是否需要启动新 context 的 watcher
    pub should_start_new: bool,
    /// 新 context ID
    pub new_context_id: String,
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

    pub fn register_context(&mut self, context: ServiceContext) -> Result<(), AppError> {
        let id = context.id.clone();
        if self.contexts.contains_key(&id) {
            return Err(AppError::Internal(format!(
                "Context '{}' already registered",
                id
            )));
        }
        self.contexts.insert(id, Arc::new(RwLock::new(context)));
        Ok(())
    }

    pub async fn replace_context(
        &mut self,
        context_id: &str,
        replacement: ServiceContext,
    ) -> Result<(), AppError> {
        if replacement.id != context_id {
            return Err(AppError::Internal(format!(
                "Replacement ID '{}' does not match '{}'",
                replacement.id, context_id
            )));
        }
        if !self.contexts.contains_key(context_id) {
            return Err(AppError::NotFound(format!(
                "Context '{}' not found",
                context_id
            )));
        }
        if let Some(old) = self.contexts.get(context_id) {
            let read_guard = old.read().await;
            let mut token_guard = read_guard.watcher_cancel_token.write().await;
            if let Some(token) = token_guard.take() {
                token.cancel();
            }
        }
        let id = replacement.id.clone();
        self.contexts.insert(id, Arc::new(RwLock::new(replacement)));
        Ok(())
    }

    pub fn switch(&mut self, target_id: &str) -> Result<SwitchResult, AppError> {
        if !self.contexts.contains_key(target_id) {
            return Err(AppError::NotFound(format!(
                "Context '{}' not found",
                target_id
            )));
        }
        // 与 Electron 对齐：切换到已激活的 context 时 no-op 成功
        let previous_id = std::mem::replace(&mut self.active_id, target_id.to_string());
        Ok(SwitchResult {
            previous_id,
            current_id: target_id.to_string(),
        })
    }

    /// 执行上下文切换，返回是否需要进行 watcher 生命周期管理。
    ///
    /// 此方法是 sync 的（与 `switch()` 一致），只负责切换 active_id 并判断
    /// 是否需要 watcher 操作。实际的 async watcher stop/start 由调用方根据
    /// 返回的 `WatcherLifecycleActions` 在合适的异步上下文中执行。
    ///
    /// # 返回值
    ///
    /// `(SwitchResult, WatcherLifecycleActions)` 元组：
    /// - `SwitchResult`: 切换结果（previous_id / current_id）
    /// - `WatcherLifecycleActions`: watcher 操作指令
    ///
    /// # 调用方职责
    ///
    /// 调用方需：
    /// 1. 已持有 `context_manager.write()` 锁（通过 RwLockWriteGuard 获得 &mut self）
    /// 2. 根据 `actions` 在同一写锁作用域内执行 async watcher 操作
    /// 3. 事件发射（Tauri emit / SSE broadcast）由调用方负责
    pub fn switch_with_watcher_actions(
        &mut self,
        context_id: &str,
    ) -> Result<(SwitchResult, WatcherLifecycleActions), AppError> {
        let result = self.switch(context_id)?;
        let actions = WatcherLifecycleActions {
            should_stop_old: result.previous_id != result.current_id,
            old_context_id: result.previous_id.clone(),
            should_start_new: result.previous_id != result.current_id,
            new_context_id: result.current_id.clone(),
        };
        Ok((result, actions))
    }

    pub async fn destroy_context(&mut self, context_id: &str) -> Result<(), AppError> {
        if context_id == "local" {
            return Err(AppError::Internal(
                "Cannot destroy the local context".into(),
            ));
        }
        let context = self
            .contexts
            .remove(context_id)
            .ok_or_else(|| AppError::NotFound(format!("Context '{}' not found", context_id)))?;
        {
            let read_guard = context.read().await;
            let mut token_guard = read_guard.watcher_cancel_token.write().await;
            if let Some(token) = token_guard.take() {
                token.cancel();
            }
        }
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

    #[allow(dead_code)]
    pub fn has(&self, context_id: &str) -> bool {
        self.contexts.contains_key(context_id)
    }

    pub fn list(&self) -> Vec<ContextInfo> {
        let mut infos: Vec<ContextInfo> = self
            .contexts
            .values()
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

    #[allow(dead_code)]
    pub async fn dispose(&mut self) {
        for ctx in self.contexts.values() {
            if let Ok(read_guard) = ctx.try_read() {
                if let Ok(mut token_guard) = read_guard.watcher_cancel_token.try_write() {
                    if let Some(token) = token_guard.take() {
                        token.cancel();
                    }
                }
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
            fs_provider: std::sync::Arc::new(
                crate::infrastructure::fs_provider::LocalFsProvider::new(),
            ),
            cache: None,
        }
    }

    #[tokio::test]
    async fn test_register_and_get() {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config(
            "local",
            ContextType::Local,
        )))
        .unwrap();
        assert!(mgr.has("local"));
        assert!(mgr.get("local").is_some());
    }

    #[tokio::test]
    async fn test_duplicate_registration_fails() {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config(
            "local",
            ContextType::Local,
        )))
        .unwrap();
        let result = mgr.register_context(ServiceContext::new(make_config(
            "local",
            ContextType::Local,
        )));
        assert!(
            matches!(result, Err(AppError::Internal(msg)) if msg.contains("already registered"))
        );
    }

    #[tokio::test]
    async fn test_switch_context() {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config(
            "local",
            ContextType::Local,
        )))
        .unwrap();
        mgr.register_context(ServiceContext::new(make_config(
            "ssh-test",
            ContextType::Ssh,
        )))
        .unwrap();
        let result = mgr.switch("ssh-test").unwrap();
        assert_eq!(result.previous_id, "local");
        assert_eq!(result.current_id, "ssh-test");
        assert_eq!(mgr.get_active_id(), "ssh-test");
    }

    #[tokio::test]
    async fn test_switch_to_nonexistent_fails() {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config(
            "local",
            ContextType::Local,
        )))
        .unwrap();
        let result = mgr.switch("nonexistent");
        assert!(matches!(result, Err(AppError::NotFound(msg)) if msg.contains("not found")));
    }

    #[tokio::test]
    async fn test_switch_to_same_is_noop() {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config(
            "local",
            ContextType::Local,
        )))
        .unwrap();
        // 与 Electron 对齐：切换到已激活 context 时 no-op 成功
        let result = mgr.switch("local");
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.previous_id, "local");
        assert_eq!(result.current_id, "local");
        assert_eq!(mgr.get_active_id(), "local");
    }

    #[tokio::test]
    async fn test_destroy_non_local() {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config(
            "local",
            ContextType::Local,
        )))
        .unwrap();
        mgr.register_context(ServiceContext::new(make_config(
            "ssh-test",
            ContextType::Ssh,
        )))
        .unwrap();
        mgr.switch("ssh-test").unwrap();
        mgr.destroy_context("ssh-test").await.unwrap();
        assert!(!mgr.has("ssh-test"));
        assert_eq!(mgr.get_active_id(), "local");
    }

    #[tokio::test]
    async fn test_destroy_local_fails() {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config(
            "local",
            ContextType::Local,
        )))
        .unwrap();
        let result = mgr.destroy_context("local").await;
        assert!(matches!(result, Err(AppError::Internal(msg)) if msg.contains("Cannot destroy")));
    }

    #[tokio::test]
    async fn test_list_returns_context_infos() {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config(
            "local",
            ContextType::Local,
        )))
        .unwrap();
        mgr.register_context(ServiceContext::new(make_config(
            "ssh-test",
            ContextType::Ssh,
        )))
        .unwrap();
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
        mgr.register_context(ServiceContext::new(make_config(
            "local",
            ContextType::Local,
        )))
        .unwrap();
        let replacement = ServiceContext::new(make_config("local", ContextType::Local));
        mgr.replace_context("local", replacement).await.unwrap();
        assert!(mgr.has("local"));
    }

    #[tokio::test]
    async fn test_replace_mismatched_id_fails() {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config(
            "local",
            ContextType::Local,
        )))
        .unwrap();
        let replacement = ServiceContext::new(make_config("wrong-id", ContextType::Local));
        let result = mgr.replace_context("local", replacement).await;
        assert!(matches!(result, Err(AppError::Internal(_))));
    }

    #[tokio::test]
    async fn test_switch_with_watcher_actions_same_context() {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config(
            "local",
            ContextType::Local,
        )))
        .unwrap();
        let (result, actions) = mgr.switch_with_watcher_actions("local").unwrap();
        assert_eq!(result.previous_id, "local");
        assert_eq!(result.current_id, "local");
        // 切换到相同 context → 不需要 watcher 操作
        assert!(!actions.should_stop_old);
        assert!(!actions.should_start_new);
    }

    #[tokio::test]
    async fn test_switch_with_watcher_actions_different_context() {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config(
            "local",
            ContextType::Local,
        )))
        .unwrap();
        mgr.register_context(ServiceContext::new(make_config(
            "ssh-test",
            ContextType::Ssh,
        )))
        .unwrap();
        let (_result, actions) = mgr.switch_with_watcher_actions("ssh-test").unwrap();
        // 切换到不同 context → 需要 stop old + start new
        assert!(actions.should_stop_old);
        assert!(actions.should_start_new);
        assert_eq!(actions.old_context_id, "local");
        assert_eq!(actions.new_context_id, "ssh-test");
    }

    #[tokio::test]
    async fn test_switch_with_watcher_actions_nonexistent_context() {
        // 错误路径测试 — 切换到不存在的 context 应返回 Err
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config(
            "local",
            ContextType::Local,
        )))
        .unwrap();
        let result = mgr.switch_with_watcher_actions("nonexistent");
        assert!(result.is_err());
        // Error 应包含上下文信息
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("nonexistent") || err_msg.to_lowercase().contains("not found"));
    }

    #[tokio::test]
    async fn test_switch_with_watcher_actions_same_context_id_precision() {
        // no-op 切换时，old_context_id 和 new_context_id 均应等于目标 ID
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config(
            "local",
            ContextType::Local,
        )))
        .unwrap();
        let (_result, actions) = mgr.switch_with_watcher_actions("local").unwrap();
        assert_eq!(actions.old_context_id, "local");
        assert_eq!(actions.new_context_id, "local");
    }

    #[tokio::test]
    async fn test_switch_with_watcher_actions_consecutive_switches() {
        // 连续切换 A→B→C：验证第二次切换的 old_context_id 为 B 而非 A
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config(
            "local",
            ContextType::Local,
        )))
        .unwrap();
        mgr.register_context(ServiceContext::new(make_config("ssh-a", ContextType::Ssh)))
            .unwrap();
        mgr.register_context(ServiceContext::new(make_config("ssh-b", ContextType::Ssh)))
            .unwrap();

        // 第一次切换: local → ssh-a
        let (_r1, a1) = mgr.switch_with_watcher_actions("ssh-a").unwrap();
        assert_eq!(a1.old_context_id, "local");
        assert_eq!(a1.new_context_id, "ssh-a");

        // 第二次切换: ssh-a → ssh-b
        let (_r2, a2) = mgr.switch_with_watcher_actions("ssh-b").unwrap();
        assert_eq!(a2.old_context_id, "ssh-a");
        assert_eq!(a2.new_context_id, "ssh-b");
    }

    #[tokio::test]
    async fn test_watcher_lifecycle_actions_clone() {
        // 验证 Clone trait 可用（调用方可能需要跨作用域传递）
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(make_config(
            "local",
            ContextType::Local,
        )))
        .unwrap();
        mgr.register_context(ServiceContext::new(make_config(
            "ssh-test",
            ContextType::Ssh,
        )))
        .unwrap();
        let (_result, actions) = mgr.switch_with_watcher_actions("ssh-test").unwrap();
        let cloned = actions.clone();
        assert_eq!(cloned.should_stop_old, actions.should_stop_old);
        assert_eq!(cloned.old_context_id, actions.old_context_id);
        assert_eq!(cloned.should_start_new, actions.should_start_new);
        assert_eq!(cloned.new_context_id, actions.new_context_id);
    }
}
