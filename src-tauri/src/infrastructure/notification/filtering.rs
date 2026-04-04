//! Filtering and throttling logic for NotificationManager.

use log::error;

use crate::parsing::git_identity::GitIdentityResolver;
use crate::types::config::DetectedError;
use crate::utils::path_decoder;

use super::{NotificationManager, THROTTLE_MS, MAX_REGEX_CACHE_SIZE};

/// Extension trait grouping filtering/throttling methods on NotificationManager.
///
/// Using a trait (rather than a bare `impl`) so that `crud.rs` can call
/// `should_notify` via `super::filtering::NotificationManagerExt::should_notify`
/// without creating circular module dependencies.
pub(super) trait NotificationManagerExt {
    async fn should_notify(&self, error: &DetectedError) -> bool;
}

impl NotificationManagerExt for NotificationManager {
    /// 判断是否应为该错误显示原生通知。
    ///
    /// 此方法仅控制操作系统通知；存储不受条件限制。
    async fn should_notify(&self, error: &DetectedError) -> bool {
        if !self.are_notifications_enabled() {
            return false;
        }

        if self.is_from_ignored_repository(error) {
            return false;
        }

        if self.matches_ignored_regex(error) {
            return false;
        }

        if self.is_throttled(error) {
            return false;
        }

        true
    }
}

impl NotificationManager {
    // =========================================================================
    // 过滤与节流
    // =========================================================================

    /// 从 projectId + message 生成节流键。
    pub(crate) fn generate_error_hash(error: &DetectedError) -> String {
        format!("{}:{}", error.project_id, error.message)
    }

    /// 检查是否因节流而抑制原生通知。
    ///
    /// 此检查不影响存储 — 仅影响原生操作系统通知。
    /// 缓存未命中时更新节流映射的时间戳。
    fn is_throttled(&self, error: &DetectedError) -> bool {
        let hash = Self::generate_error_hash(error);
        let now = super::emission::now_millis();

        let mut throttle_map = match self.throttle_map.lock() {
            Ok(m) => m,
            Err(e) => {
                error!("Failed to acquire throttle lock: {e}");
                return false;
            }
        };

        if let Some(&last_seen) = throttle_map.get(&hash) {
            if now - last_seen < THROTTLE_MS {
                return true;
            }
        }

        // 更新节流映射
        throttle_map.insert(hash, now);

        // 定期清理过期条目
        let expired_threshold = now.saturating_sub(THROTTLE_MS * 2);
        throttle_map.retain(|_, ts| *ts >= expired_threshold);

        false
    }

    /// 检查错误消息是否匹配任何已配置的忽略正则模式。
    fn matches_ignored_regex(&self, error: &DetectedError) -> bool {
        let config = self.config_manager.get_config();

        if config.notifications.ignored_regex.is_empty() {
            return false;
        }

        for pattern in &config.notifications.ignored_regex {
            let case_insensitive = format!("(?i){}", pattern);

            // 查缓存
            {
                let cache = self.regex_cache.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(cached) = cache.get(&case_insensitive) {
                    if let Some(ref re) = cached {
                        if re.is_match(&error.message) {
                            return true;
                        }
                    }
                    continue; // cached as None (invalid regex)
                }
            }

            // 未命中缓存，编译并缓存
            let compiled = crate::utils::regex_validation::create_safe_regex(&case_insensitive);
            let is_match = compiled.as_ref().map_or(false, |re| re.is_match(&error.message));

            {
                let mut cache = self.regex_cache.lock().unwrap_or_else(|e| e.into_inner());
                if cache.len() >= MAX_REGEX_CACHE_SIZE {
                    cache.clear();
                }
                cache.insert(case_insensitive, compiled);
            }

            if is_match {
                return true;
            }
        }

        false
    }

    /// Check if the error comes from an ignored repository.
    ///
    /// Resolves the error's projectId to a repository identity via GitIdentityResolver,
    /// then checks against config.notifications.ignored_repositories.
    fn is_from_ignored_repository(&self, error: &DetectedError) -> bool {
        let config = self.config_manager.get_config();
        let ignored = &config.notifications.ignored_repositories;
        if ignored.is_empty() {
            return false;
        }

        let resolver = GitIdentityResolver;
        let project_path = path_decoder::decode_path(&error.project_id);
        let resolved_path = error.context.cwd.as_deref().unwrap_or(&project_path);

        match resolver.resolve_identity(resolved_path) {
            Some(identity) => ignored.contains(&identity.id),
            None => false,
        }
    }

    /// 检查通知当前是否启用（未暂停、未禁用）。
    fn are_notifications_enabled(&self) -> bool {
        let config = self.config_manager.get_config();

        if !config.notifications.enabled {
            return false;
        }

        if let Some(snoozed_until) = config.notifications.snoozed_until {
            let now = super::emission::now_millis();
            if now < snoozed_until {
                return false;
            }
            // 暂停已过期 — 清除暂停状态
            self.config_manager.clear_snooze();
            return true;
        }

        true
    }
}
