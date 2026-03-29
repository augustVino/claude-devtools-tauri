//! 上下文管理器 — 占位文件，Task 2 中完整实现。

/// 上下文元数据。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ContextInfo {
    pub id: String,
    #[serde(rename = "type")]
    pub context_type: String,
}

/// 上下文管理器（占位）。
pub struct ContextManager {
    active_id: String,
}

impl ContextManager {
    pub fn new() -> Self {
        Self {
            active_id: "local".to_string(),
        }
    }

    pub fn get_active_id(&self) -> &str {
        &self.active_id
    }
}

impl Default for ContextManager {
    fn default() -> Self {
        Self::new()
    }
}
