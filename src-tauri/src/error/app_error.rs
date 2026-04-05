//! 应用级类型化错误。
//!
//! 替代全项目散布的 `Result<T, String>` 错误处理模式。
//! 在 service/infrastructure 内部使用，在 Command/Route 边界转换为 String。

use thiserror::Error;

/// 应用错误枚举 — 覆盖所有主要错误类别。
#[derive(Error, Debug)]
pub enum AppError {
    /// 资源未找到
    #[error("Not found: {0}")]
    NotFound(String),

    /// 输入参数无效
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    /// IO 错误（自动从 std::io::Error 转换）
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// SSH/SFTP 错误
    #[error("SSH/SFTP error: {0}")]
    #[allow(dead_code)]
    Ssh(String),

    /// 解析错误（JSONL 解析失败等）
    #[error("Parse error: {0}")]
    Parse(String),

    /// 配置错误
    #[error("Config error: {0}")]
    Config(String),

    /// 内部错误（不应发生的逻辑错误）
    #[error("Internal error: {0}")]
    #[allow(dead_code)]
    Internal(String),
}

impl AppError {
    /// 将 AppError 转换为 Tauri command 兼容的 String 格式。
    /// 用于 Command 层边界：`.map_err(AppError::to_string)`
    #[allow(dead_code)]
    pub fn into_tauri_string(self) -> String {
        self.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_not_found_format() {
        let err = AppError::NotFound("session abc".into());
        assert_eq!(err.to_string(), "Not found: session abc");
    }

    #[test]
    fn test_io_from_conversion() {
        let err = AppError::from(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "missing",
        ));
        assert!(err.to_string().contains("IO error"));
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn test_into_tauri_string() {
        let err = AppError::InvalidInput("bad id".into());
        assert_eq!(err.into_tauri_string(), "Invalid input: bad id");
    }
}
