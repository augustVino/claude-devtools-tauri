//! SSH Service — SSH 连接的完整生命周期编排。
//!
//! connect / disconnect 是编排方法（Orchestrator），协调 6+ 个基础设施组件：
//! SshConnectionManager, ContextManager, ConfigManager, NotificationManager,
//! ServiceContext, SSEBroadcaster。

use crate::error::AppError;
use crate::http::sse::SSEBroadcaster;
use crate::types::ssh::{
    SshConfigHostEntry, SshConnectionConfig, SshConnectionStatus, SshTestResult,
};
use async_trait::async_trait;

/// SSH 连接操作的返回值类型别名。
pub type SshConnectResult = SshConnectionStatus;

#[async_trait]
pub trait SshService: Send + Sync {
    // ── 核心编排方法（Orchestrator）──

    /// 建立 SSH 连接并完成完整的上下文切换生命周期。
    ///
    /// `AppHandle` 由 impl struct 在构造时存储（H3 设计决策），
    /// 不再作为参数传递以简化签名并兼容 HTTP path 调用。
    async fn connect(
        &self,
        config: SshConnectionConfig,
        broadcaster: Option<&SSEBroadcaster>,
    ) -> Result<SshConnectResult, AppError>;

    /// 断开 SSH 连接并完成上下文回切生命周期。
    ///
    /// 行为约定：始终执行 disconnect（幂等操作）。
    /// 若当前不在 SSH context 上，跳过 context switch 流程但仍然断连。
    async fn disconnect(
        &self,
        broadcaster: Option<&SSEBroadcaster>,
    ) -> Result<SshConnectionStatus, AppError>;

    /// 处理 health monitor 检测到的远程断开（默认 no-op，SshServiceImpl 实现具体逻辑）。
    ///
    /// 默认实现返回 Ok(()) 以支持未来 mock impl 不需强制覆盖。
    async fn handle_remote_disconnect(
        &self,
        _broadcaster: Option<&crate::http::sse::SSEBroadcaster>,
    ) -> Result<(), AppError> {
        Ok(())
    }

    // ── 只读查询 ──
    //
    // 【编译约束】以下方法虽然概念上是"只读"，但内部需要通过
    // `self.ssh_manager.read().await` 获取 RwLockReadGuard，因此必须声明为 async fn。
    // 如果声明为 fn（sync），impl 中使用 .await 将导致编译错误。

    /// 获取当前活跃连接状态（异步，直接返回值）
    async fn get_active_state(&self) -> SshConnectionStatus;

    /// 测试 SSH 连接可达性
    async fn test(&self, config: &SshConnectionConfig) -> Result<SshTestResult, AppError>;

    /// 获取 SSH config 中所有 host 条目
    async fn get_config_hosts(&self) -> Vec<SshConfigHostEntry>;

    /// 按 alias 解析 SSH config host 条目
    async fn resolve_host_config(&self, alias: &str) -> Option<SshConfigHostEntry>;

    // ── 辅助方法（无状态，但需 &self 以保持 object-safety）──

    /// 从 host 名构造 SSH context ID
    fn ssh_context_id(&self, host: &str) -> String {
        if host.is_empty() {
            "ssh".to_string()
        } else {
            format!("ssh-{host}")
        }
    }

    /// 判断 context ID 是否属于 SSH 上下文
    fn is_ssh_context_id(&self, id: &str) -> bool {
        id == "ssh" || id.starts_with("ssh-")
    }
}

#[cfg(test)]
mod tests_context_id {
    use super::*;

    /// 辅助：用最小 mock impl 触发 trait default method（is_ssh_context_id）
    /// 无需 mock 任何 async method（is_ssh_context_id 是 sync default method）
    struct DummySshService;

    #[async_trait]
    impl SshService for DummySshService {
        async fn connect(
            &self,
            _: SshConnectionConfig,
            _: Option<&SSEBroadcaster>,
        ) -> Result<SshConnectResult, AppError> {
            unreachable!("test only triggers default method")
        }
        async fn disconnect(
            &self,
            _: Option<&SSEBroadcaster>,
        ) -> Result<SshConnectionStatus, AppError> {
            unreachable!()
        }
        async fn get_active_state(&self) -> SshConnectionStatus {
            unreachable!()
        }
        async fn test(&self, _: &SshConnectionConfig) -> Result<SshTestResult, AppError> {
            unreachable!()
        }
        async fn get_config_hosts(&self) -> Vec<SshConfigHostEntry> {
            unreachable!()
        }
        async fn resolve_host_config(&self, _: &str) -> Option<SshConfigHostEntry> {
            unreachable!()
        }
    }

    #[test]
    fn test_is_ssh_context_id_matches_ssh_prefix() {
        let dummy = DummySshService;
        assert!(dummy.is_ssh_context_id("ssh"));
        assert!(dummy.is_ssh_context_id("ssh-myserver"));
        assert!(dummy.is_ssh_context_id("ssh-192.168.1.1"));
    }

    #[test]
    fn test_is_ssh_context_id_rejects_local_and_others() {
        let dummy = DummySshService;
        assert!(!dummy.is_ssh_context_id("local"));
        assert!(!dummy.is_ssh_context_id(""));
        assert!(!dummy.is_ssh_context_id("sshlite")); // 前缀必须是 "ssh-" 或恰好 "ssh"
    }
}
