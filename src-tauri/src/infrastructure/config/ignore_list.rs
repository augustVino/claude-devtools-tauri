//! 忽略列表管理（正则 + 仓库）。

impl super::ConfigManager {
    /// 添加一个正则表达式到忽略列表。会校验正则语法和去重。
    pub async fn add_ignore_regex(&self, pattern: String) -> Result<crate::types::AppConfig, String> {
        let trimmed = pattern.trim().to_string();
        if trimmed.is_empty() { return Err("pattern must not be empty".into()); }
        if let Err(e) = regex::Regex::new(&trimmed) { return Err(format!("invalid regex pattern: {e}")); }
        let mut config = self.config.write().await;
        if config.notifications.ignored_regex.contains(&trimmed) { return Err("pattern already exists".into()); }
        config.notifications.ignored_regex.push(trimmed);
        drop(config); self.persist().await?; Ok(self.get_config().await)
    }

    /// 从忽略列表中移除指定的正则表达式
    pub async fn remove_ignore_regex(&self, pattern: String) -> crate::types::AppConfig {
        if let Ok(mut config) = self.config.try_write() {
            config.notifications.ignored_regex.retain(|p| p != &pattern);
            drop(config); let _ = self.persist().await;
        }
        self.get_config().await
    }

    /// 添加一个仓库 ID 到忽略列表
    pub async fn add_ignore_repository(&self, repo_id: String) -> crate::types::AppConfig {
        let trimmed = repo_id.trim().to_string();
        if trimmed.is_empty() { return self.get_config().await; }
        if let Ok(mut config) = self.config.try_write() {
            if !config.notifications.ignored_repositories.contains(&trimmed) {
                config.notifications.ignored_repositories.push(trimmed);
                drop(config); let _ = self.persist().await;
            }
        }
        self.get_config().await
    }

    /// 从忽略列表中移除指定的仓库 ID
    pub async fn remove_ignore_repository(&self, repo_id: String) -> crate::types::AppConfig {
        if let Ok(mut config) = self.config.try_write() {
            config.notifications.ignored_repositories.retain(|id| id != &repo_id);
            drop(config); let _ = self.persist().await;
        }
        self.get_config().await
    }
}
