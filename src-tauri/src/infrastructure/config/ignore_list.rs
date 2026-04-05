//! 忽略列表管理（正则 + 仓库）。

use crate::error::AppError;

impl super::ConfigManager {
    /// 添加一个正则表达式到忽略列表。会校验正则语法和去重。
    pub async fn add_ignore_regex(&self, pattern: String) -> Result<crate::types::AppConfig, AppError> {
        let trimmed = Self::validate_ignore_pattern(pattern)?;

        self.with_config_mut(|config| {
            if config.notifications.ignored_regex.contains(&trimmed) {
                return false; // 幂等：重复视为无变更
            }
            config.notifications.ignored_regex.push(trimmed);
            true
        }).await
    }

    /// 从忽略列表中移除指定的正则表达式
    pub async fn remove_ignore_regex(&self, pattern: String) -> Result<crate::types::AppConfig, AppError> {
        self.with_config_mut(|config| {
            let before = config.notifications.ignored_regex.len();
            config.notifications.ignored_regex.retain(|p| p != &pattern);
            config.notifications.ignored_regex.len() != before
        }).await
    }

    /// 添加一个仓库 ID 到忽略列表
    pub async fn add_ignore_repository(&self, repo_id: String) -> Result<crate::types::AppConfig, AppError> {
        let trimmed = repo_id.trim().to_string();
        if trimmed.is_empty() {
            return Ok(self.get_config().await);
        }

        self.with_config_mut(|config| {
            if !config.notifications.ignored_repositories.contains(&trimmed) {
                config.notifications.ignored_repositories.push(trimmed);
                true
            } else { false }
        }).await
    }

    /// 从忽略列表中移除指定的仓库 ID
    pub async fn remove_ignore_repository(&self, repo_id: String) -> Result<crate::types::AppConfig, AppError> {
        self.with_config_mut(|config| {
            let before = config.notifications.ignored_repositories.len();
            config.notifications.ignored_repositories.retain(|id| id != &repo_id);
            config.notifications.ignored_repositories.len() != before
        }).await
    }

    /// 校验忽略正则模式（纯函数，无副作用）。
    fn validate_ignore_pattern(pattern: String) -> Result<String, AppError> {
        let trimmed = pattern.trim().to_string();
        if trimmed.is_empty() {
            return Err(AppError::InvalidInput("pattern must not be empty".into()));
        }
        regex::Regex::new(&trimmed)
            .map_err(|e| AppError::InvalidInput(format!("invalid regex pattern: {e}")))?;
        Ok(trimmed)
    }
}
