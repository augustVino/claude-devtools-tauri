//! Config Service — 含编排逻辑的配置操作。
//!
//! ConfigManager 本身是数据访问对象（CRUD）。本 Service 覆盖包含
//! 多步操作 + 条件分支 + 副作用的编排逻辑：
//! - update_config: claude root 变更检测 + rebuild
//! - snooze: 参数校验分支
//! - test_trigger: ProjectScanner 构建 + 触发测试
//! - clear_snooze: 简单委托

use crate::error::AppError;
use crate::types::config::{AppConfig, NotificationTrigger, TriggerTestResult};
use async_trait::async_trait;

#[async_trait]
pub trait ConfigService: Send + Sync {
    /// 更新配置，自动检测 claude root 变更并触发上下文重建。
    ///
    /// rebuild 失败不阻塞配置更新本身（记录 error 日志），
    /// 统一了 IPC（原：rebuild 失败抛错阻塞更新）和 HTTP（原：静默吞掉）
    /// 两种不一致的行为。（Bug B1 修复）
    async fn update_config(
        &self,
        section: &str,
        data: serde_json::Value,
    ) -> Result<AppConfig, AppError>;

    /// 免打扰设置（含参数校验：-1=明天, <=0=非法, >1440=非法）
    async fn snooze(&self, minutes: i32) -> Result<AppConfig, AppError>;
    async fn clear_snooze(&self) -> Result<AppConfig, AppError>;

    /// 测试通知触发器。
    async fn test_trigger(
        &self,
        trigger: &NotificationTrigger,
    ) -> Result<TriggerTestResult, AppError>;
}
