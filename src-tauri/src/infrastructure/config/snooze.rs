//! 通知暂停（snooze）管理。

use crate::error::AppError;

impl super::ConfigManager {
    /// 暂停通知指定分钟数
    pub async fn snooze(&self, minutes: u32) -> Result<crate::types::AppConfig, AppError> {
        let snoozed_until = super::defaults::now_millis() + (minutes as u64) * 60 * 1000;
        self.with_config_mut(|config| {
            config.notifications.snoozed_until = Some(snoozed_until);
            true
        }).await
    }

    /// Snooze notifications until midnight tomorrow.
    pub async fn snooze_until_tomorrow(&self) -> Result<crate::types::AppConfig, AppError> {
        let tomorrow = chrono::Local::now().date_naive() + chrono::Duration::days(1);
        let tomorrow_midnight = tomorrow.and_hms_opt(0, 0, 0).unwrap()
            .and_local_timezone(chrono::Local).single().unwrap_or_else(|| {
                tomorrow.and_hms_opt(12, 0, 0).unwrap()
                    .and_local_timezone(chrono::Local).single()
                    .expect("noon should never be ambiguous")
            });
        let snoozed_until = tomorrow_midnight.timestamp_millis() as u64;
        self.with_config_mut(|config| {
            config.notifications.snoozed_until = Some(snoozed_until);
            true
        }).await
    }

    /// 清除通知暂停状态，恢复通知
    pub async fn clear_snooze(&self) -> Result<crate::types::AppConfig, AppError> {
        self.with_config_mut(|config| {
            config.notifications.snoozed_until = None;
            true
        }).await
    }
}
