//! 通知暂停（snooze）管理。

use chrono;

impl super::ConfigManager {
    /// 暂停通知指定分钟数
    pub async fn snooze(&self, minutes: u32) -> crate::types::AppConfig {
        let snoozed_until = super::defaults::now_millis() + (minutes as u64) * 60 * 1000;
        let mut config = self.config.write().await;
        config.notifications.snoozed_until = Some(snoozed_until);
        drop(config); let _ = self.persist().await;
        log::info!("Notifications snoozed for {minutes} minutes");
        self.get_config().await
    }

    /// Snooze notifications until midnight tomorrow.
    pub async fn snooze_until_tomorrow(&self) -> crate::types::AppConfig {
        let tomorrow = chrono::Local::now().date_naive() + chrono::Duration::days(1);
        let tomorrow_midnight = tomorrow.and_hms_opt(0, 0, 0).unwrap()
            .and_local_timezone(chrono::Local).single().unwrap_or_else(|| {
                tomorrow.and_hms_opt(12, 0, 0).unwrap()
                    .and_local_timezone(chrono::Local).single()
                    .expect("noon should never be ambiguous")
            });
        let snoozed_until = tomorrow_midnight.timestamp_millis() as u64;
        let mut config = self.config.write().await;
        config.notifications.snoozed_until = Some(snoozed_until);
        drop(config); let _ = self.persist().await;
        log::info!("Notifications snoozed until tomorrow midnight");
        self.get_config().await
    }

    /// 清除通知暂停状态，恢复通知
    pub async fn clear_snooze(&self) -> crate::types::AppConfig {
        let mut config = self.config.write().await;
        config.notifications.snoozed_until = None;
        drop(config); let _ = self.persist().await;
        log::info!("Snooze cleared");
        self.get_config().await
    }
}
