//! SshServiceImpl — SSH 连接生命周期的具体实现。
//!
//! ## 设计决策（H3）
//!
//! `AppHandle` 在构造时存储（而非作为方法参数），原因：
//! 1. connect/disconnect/test 等所有方法都可能需要 emit 事件或获取 SSE broadcaster
//! 2. HTTP path（Batch 3）通过 HttpState 调用时，AppHandle 不方便作为参数传递
//! 3. AppHandle 本身是 Clone 的轻量句柄，存储开销可忽略

use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock;

use crate::error::AppError;
use crate::http::sse::{BackendEvent, SSEBroadcaster};
use crate::infrastructure::{
    context_manager::ContextInfo,
    service_context::{ContextType, ServiceContext, ServiceContextConfig},
    ConfigManager, ContextManager, DataCache, NotificationManager, SshConnectionManager,
};
use crate::services::ssh_service_trait::{SshConnectResult, SshService};
use crate::types::ssh::{
    SshConfigHostEntry, SshConnectionConfig, SshConnectionStatus, SshTestResult,
};

pub struct SshServiceImpl {
    ssh_manager: Arc<RwLock<SshConnectionManager>>,
    context_manager: Arc<RwLock<ContextManager>>,
    // DataCache 已有 #[derive(Clone)]（内部基于 Arc），无需再包 Arc
    cache: DataCache,
    config_manager: Arc<ConfigManager>,
    notification_manager: Arc<RwLock<NotificationManager>>,
    /// 存储的 AppHandle，用于事件发射和 SSE broadcaster 获取。
    /// 替代原设计中将 AppHandle 作为每个方法参数传递的方式。
    app_handle: AppHandle,
}

impl SshServiceImpl {
    pub fn new(
        ssh_manager: Arc<RwLock<SshConnectionManager>>,
        context_manager: Arc<RwLock<ContextManager>>,
        cache: DataCache,
        config_manager: Arc<ConfigManager>,
        notification_manager: Arc<RwLock<NotificationManager>>,
        app_handle: AppHandle,
    ) -> Self {
        Self {
            ssh_manager,
            context_manager,
            cache,
            config_manager,
            notification_manager,
            app_handle,
        }
    }

    /// 执行 watcher 生命周期操作（stop old + start new）。
    ///
    /// ## Lock 约定（H2 统一）
    ///
    /// 原始代码中 connect 和 disconnect 的 lock 释放时序不一致：
    /// - connect: drop(mgr) → emit → SSE （先释放锁）
    /// - disconnect: 持锁穿过 emit/SSE （后释放锁）
    ///
    /// **统一策略**：本 helper 仅执行 watcher stop/start（需写锁），
    /// 不负责 emit/SSE。调用方在 drop 写锁后再执行 emit/SSE，
    /// 确保锁持有时间最短且行为一致。
    async fn execute_watcher_lifecycle(
        &self,
        mgr: &mut crate::infrastructure::ContextManager,
        actions: &crate::infrastructure::context_manager::WatcherLifecycleActions,
    ) {
        if actions.should_stop_old {
            if let Some(old_ctx) = mgr.get(&actions.old_context_id) {
                old_ctx.read().await.stop_watcher_tasks().await;
            }
        }
        if actions.should_start_new {
            if let Some(new_ctx) = mgr.get(&actions.new_context_id) {
                let new = new_ctx.read().await;
                new.spawn_watcher_tasks(
                    self.app_handle.clone(),
                    self.config_manager.clone(),
                    self.notification_manager.clone(),
                )
                .await;
            }
        }
    }

    /// 统一的事件发射逻辑（connect / disconnect 共享）。
    ///
    /// 约定：必须在 **write lock 已 drop 后** 调用。
    ///
    /// **Best-effort**（v3-N5）：Tauri emit 失败（如前端无窗口、IPC 通道关闭）
    /// 不再当致命错误。降级为 log::warn!，避免阻塞 SSH 连接断开导致资源泄漏。
    fn emit_context_changed(&self, info: ContextInfo, broadcaster: Option<&SSEBroadcaster>) {
        if let Err(e) = self.app_handle.emit("context:changed", &info) {
            log::warn!("emit context:changed failed (non-fatal): {}", e);
        }
        if let Some(bcast) = broadcaster {
            bcast.send(BackendEvent::ContextChanged(info));
        }
    }

    /// 步骤 1：建立 SSH 网络连接（纯网络操作）
    async fn connect_ssh(
        &self,
        config: SshConnectionConfig,
    ) -> Result<SshConnectionStatus, AppError> {
        self.ssh_manager
            .write()
            .await
            .connect(config)
            .await
            .map_err(AppError::Ssh)
    }

    /// 步骤 2：根据连接状态构建 SSH ServiceContext（纯数据组装）
    ///
    /// 【审查发现 #1】必须是 async fn——内部使用了 .await
    /// （获取 RwLockReadGuard + 调用 async 的 get_provider）。
    async fn build_ssh_context(
        &self,
        status: &SshConnectionStatus,
        username: &str,
    ) -> Result<ServiceContext, AppError> {
        let host = status.host.clone().unwrap_or_default();
        let remote_projects_path = status
            .remote_projects_path
            .clone()
            .unwrap_or_else(|| format!("/home/{}/.claude/projects", username));
        let remote_todos_path = PathBuf::from(&remote_projects_path)
            .parent()
            .map(|p| p.join("todos"))
            .unwrap_or_else(|| PathBuf::from("/tmp/claude-todos-ssh"));

        let fs_provider: Arc<dyn crate::infrastructure::FsProvider> = {
            let mgr = self.ssh_manager.read().await;
            mgr.get_provider().await.ok_or_else(|| {
                AppError::Internal("SSH provider not available after connect".into())
            })?
        };

        Ok(ServiceContext::new(ServiceContextConfig {
            id: self.ssh_context_id(&host),
            context_type: ContextType::Ssh,
            home_dir: Some(PathBuf::new()),
            projects_dir: PathBuf::from(&remote_projects_path),
            todos_dir: remote_todos_path,
            fs_provider,
            cache: Some(self.cache.clone()),
        }))
    }

    /// 切回 local context（从 disconnect 提取，供主动 disconnect 与 health monitor 断开复用）。
    ///
    /// 幂等（TOCTOU 安全）：读锁检查后，写锁内**再次检查** is_ssh_context_id，
    /// 避免双重 switch（用户主动 disconnect + health monitor 并发触发时，
    /// 第二次进入写锁块会因 destroy_context("local") 返回 Err 污染日志）。
    ///
    /// **destroy non-fatal**（v3-N6）：destroy_context 失败时降级为 warn。
    /// destroy 失败仅两种情形（v4-m4）：
    ///   - context_id == "local"：destroy_context 内部拒绝（不应发生，switch_to_local 后 previous_id 是 ssh-x）
    ///   - contexts.remove 返回 None：HashMap 本就无该条目，**无实际残留**
    /// 两种情形均不阻塞 SSH 连接断开。
    async fn switch_to_local_context(&self) -> Result<Option<ContextInfo>, AppError> {
        let is_ssh_active = {
            let mgr = self.context_manager.read().await;
            self.is_ssh_context_id(mgr.get_active_id())
        };

        if !is_ssh_active {
            return Ok(None);
        }

        let info = {
            let mut mgr = self.context_manager.write().await;

            // TOCTOU 安全：写锁内再次检查（v2 H7）
            // 读锁释放到写锁获取之间，其他 caller 可能已切回 local
            if !self.is_ssh_context_id(mgr.get_active_id()) {
                log::debug!(
                    "switch_to_local_context: already local after acquiring write lock, skip"
                );
                return Ok(None);
            }

            let (result, actions) = mgr.switch_with_watcher_actions("local")?;
            log::info!(
                "SSH context switch to local: {} -> {}",
                result.previous_id,
                result.current_id
            );

            self.execute_watcher_lifecycle(&mut mgr, &actions).await;

            // destroy non-fatal：失败降级 warn，不阻塞（v3-N6）
            if let Err(e) = mgr.destroy_context(&result.previous_id).await {
                log::warn!(
                    "destroy_context({}) failed (non-fatal): {}",
                    result.previous_id,
                    e
                );
            }

            let ctx_arc = mgr
                .get(&result.current_id)
                .ok_or_else(|| AppError::Internal("Local context not found after switch".into()))?;
            let info = ContextInfo::from_context(&*ctx_arc.read().await);

            drop(mgr);
            info
        };

        Ok(Some(info))
    }
}

#[async_trait]
impl SshService for SshServiceImpl {
    // ── Orchestrator: connect ──

    async fn connect(
        &self,
        config: SshConnectionConfig,
        broadcaster: Option<&SSEBroadcaster>,
    ) -> Result<SshConnectResult, AppError> {
        let username = config.username.clone();

        // 1. Establish SSH connection.
        // Task 8: SshConnectionManager::connect 失败时返回 Err(String)，
        // connect_ssh 已把它包装为 AppError::Ssh，这里通过 `?` 传播。
        // 因此到达此行时 status.state 只可能是 Connected，无需再检查 Error 早期返回。
        let status = self.connect_ssh(config).await?;

        // 2. Build SSH ServiceContext
        let ctx = self.build_ssh_context(&status, &username).await?;

        // 3+4. Single write-lock block（D3 fix: merged from original two acquisitions）
        //
        // 原 design 有两处 context_manager.write().await 获取：
        //   - Block A: 检查是否已在 SSH context → tear down → switch to local
        //   - Block B: register new SSH context → switch to it
        // 问题：两次获取之间存在 TOCTOU 窗口。
        // 修正：合并为单次写锁块，消除竞态窗口。
        let info = {
            let mut mgr = self.context_manager.write().await;

            // 3a. If already on SSH context, tear down first
            if self.is_ssh_context_id(mgr.get_active_id()) {
                log::info!("SSH connect: already on SSH context, tearing down before reconnect");
                let old_ssh_id = mgr.get_active_id().to_string();
                if let Some(ssh_ctx) = mgr.get(&old_ssh_id) {
                    ssh_ctx.read().await.stop_watcher_tasks().await;
                }
                if let Ok(result) = mgr.switch("local") {
                    Self::execute_watcher_lifecycle(
                        self,
                        &mut mgr,
                        &crate::infrastructure::context_manager::WatcherLifecycleActions {
                            should_stop_old: result.previous_id != result.current_id,
                            old_context_id: result.previous_id.clone(),
                            should_start_new: result.previous_id != result.current_id,
                            new_context_id: result.current_id.clone(),
                        },
                    )
                    .await;
                }
                let _ = mgr.destroy_context(&old_ssh_id).await;
            }

            // 4. Register SSH context and switch
            mgr.register_context(ctx)
                .map_err(|e| AppError::Internal(e.to_string()))?;

            let ctx_id = self.ssh_context_id(&status.host.clone().unwrap_or_default());
            let (result, actions) = mgr.switch_with_watcher_actions(&ctx_id)?;
            log::info!(
                "SSH connect: context switched {} -> {}",
                result.previous_id,
                result.current_id
            );

            // Execute watcher lifecycle（仍持写锁）
            self.execute_watcher_lifecycle(&mut mgr, &actions).await;

            // 获取 ContextInfo 用于后续 emit（仍持写锁）
            let ctx_arc = mgr
                .get(&result.current_id)
                .ok_or_else(|| AppError::Internal("SSH context not found after switch".into()))?;
            let info = ContextInfo::from_context(&*ctx_arc.read().await);

            // ★ 关键：在此 drop 写锁，之后的所有 I/O（emit/SSE）无锁执行
            drop(mgr);

            info // 返回 info 给外部使用
        };

        // Emit event + SSE（无锁状态，H2 统一策略）
        self.emit_context_changed(info, broadcaster);

        Ok(status)
    }

    // ── Orchestrator: disconnect ──

    async fn disconnect(
        &self,
        broadcaster: Option<&SSEBroadcaster>,
    ) -> Result<SshConnectionStatus, AppError> {
        // 复用提取的 context switch 逻辑（DRY + TOCTOU + non-fatal destroy）
        if let Some(info) = self.switch_to_local_context().await? {
            // emit non-fatal（v3-N5）：失败不阻塞 ssh_manager.disconnect()
            self.emit_context_changed(info, broadcaster);
        }

        // Disconnect SSH connection (always execute — 幂等操作)
        let status = self
            .ssh_manager
            .write()
            .await
            .disconnect()
            .await
            .map_err(AppError::Ssh)?;
        Ok(status)
    }

    /// 处理 health monitor 检测到的远程断开（对齐 Electron handleDisconnect 响应动作）。
    ///
    /// 与用户主动 disconnect 的区别：不调 ssh_manager.disconnect()（health monitor
    /// 已 cleanup）；只做 context switch + emit。
    ///
    /// 检测机制（health monitor task + broadcast）保留 Tauri 增强，
    /// 非 Electron 的 client.on('end') 事件——响应动作对齐即可。
    async fn handle_remote_disconnect(
        &self,
        broadcaster: Option<&SSEBroadcaster>,
    ) -> Result<(), AppError> {
        if let Some(info) = self.switch_to_local_context().await? {
            log::info!("SSH remote disconnect detected by health monitor, switched to local");
            self.emit_context_changed(info, broadcaster);
        }
        Ok(())
    }

    // ── 只读查询 ──

    async fn get_active_state(&self) -> SshConnectionStatus {
        self.ssh_manager.read().await.get_active_state().await
    }

    async fn test(&self, config: &SshConnectionConfig) -> Result<SshTestResult, AppError> {
        self.ssh_manager
            .read()
            .await
            .test(config)
            .await
            .map_err(AppError::Ssh)
    }

    async fn get_config_hosts(&self) -> Vec<SshConfigHostEntry> {
        self.ssh_manager.read().await.get_config_hosts()
    }

    async fn resolve_host_config(&self, alias: &str) -> Option<SshConfigHostEntry> {
        self.ssh_manager.read().await.resolve_host_config(alias)
    }
}
