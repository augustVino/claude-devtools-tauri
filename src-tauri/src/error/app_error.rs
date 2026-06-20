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
    Ssh(String),

    /// 解析错误（JSONL 解析失败等）
    #[error("Parse error: {0}")]
    Parse(String),

    /// 配置错误
    #[error("Config error: {0}")]
    Config(String),

    /// 内部错误（不应发生的逻辑错误）
    #[error("Internal error: {0}")]
    Internal(String),

    /// 锁错误（Mutex/RwLock PoisonError）
    #[error("Lock error: {0}")]
    LockError(String),

    /// 任务取消（tokio::task::JoinError）
    #[error("Task cancelled: {0}")]
    Cancelled(String),

    /// 序列化/反序列化错误（serde_json）
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// 文件操作错误（FsProvider 子阶段使用）
    #[error("File operation failed: {0}")]
    FileOp(String),

    /// 外部应用打开失败（open -a 命令失败、应用未安装等）
    #[error("OpenFailed: {0}")]
    OpenFailed(String),
}

impl AppError {
    /// 将 AppError 转换为 Tauri command 兼容的 String 格式。
    /// 用于 Command 层边界：`.map_err(AppError::to_string)`
    pub fn into_tauri_string(self) -> String {
        self.to_string()
    }
}

// serde_json::Error → AppError::Serialization
impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Serialization(e.to_string())
    }
}

// tokio JoinError → AppError::Cancelled
impl From<tokio::task::JoinError> for AppError {
    fn from(e: tokio::task::JoinError) -> Self {
        AppError::Cancelled(e.to_string())
    }
}

// Mutex PoisonError → AppError::LockError
impl<T> From<std::sync::PoisonError<T>> for AppError {
    fn from(e: std::sync::PoisonError<T>) -> Self {
        AppError::LockError(format!("mutex poisoned: {}", e))
    }
}

// ── Axum IntoResponse（保持 HTTP 200 约定）──
//
// 让 HTTP route handler 可以使用 `Result<T, AppError>` 返回风格，
// 消除每个函数中 `match { Ok => ..., Err => error_json(...) }` 的样板。
// 所有错误仍返回 HTTP 200 + `{ success: false, error: "..." }`，
// 与现有前端契约完全兼容。
//
// **注意**: 此 impl 被 `#[cfg(feature = "http")]` 守卫。纯 Tauri IPC 构建
// 可通过 `--no-default-features` 排除 axum 及其传递依赖（hyper/tower 等）。
// 若修改此处 JSON 结构，需同步更新 `http::routes::ErrorResponse`。
#[cfg(feature = "http")]
impl axum::response::IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        use axum::{http::StatusCode, Json};
        // 为避免循环依赖（error → http → routes → error），
        // 此处内联构造等价结构，不引用 http::routes::ErrorResponse
        let body = serde_json::json!({
            "success": false,
            "error": self.to_string(),
        });
        (StatusCode::OK, Json(body)).into_response()
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
        let err = AppError::from(std::io::Error::new(std::io::ErrorKind::NotFound, "missing"));
        assert!(err.to_string().contains("IO error"));
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn test_into_tauri_string() {
        let err = AppError::InvalidInput("bad id".into());
        assert_eq!(err.into_tauri_string(), "Invalid input: bad id");
    }

    #[test]
    fn test_serialization_from_json_error() {
        let err = AppError::from(serde_json::from_str::<serde_json::Value>("invalid").unwrap_err());
        assert!(err.to_string().contains("Serialization error"));
    }

    #[test]
    fn test_cancelled_format() {
        let err = AppError::Cancelled("task cancelled".into());
        assert_eq!(err.to_string(), "Task cancelled: task cancelled");
    }

    #[test]
    fn test_lock_error_format() {
        let err = AppError::LockError("mutex poisoned".into());
        assert_eq!(err.to_string(), "Lock error: mutex poisoned");
    }

    #[cfg(feature = "http")]
    #[tokio::test]
    async fn test_into_response_returns_200_with_error_body() {
        use axum::response::IntoResponse;

        let err = AppError::NotFound("resource xyz".into());
        let response = err.into_response();

        // Status must be HTTP 200（与现有前端契约对齐）
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        // Body 必须是 JSON 且包含 success:false + error message
        let body_bytes = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["success"], false);
        assert!(body["error"].as_str().unwrap().contains("resource xyz"));
    }

    #[cfg(feature = "http")]
    #[tokio::test]
    async fn test_into_response_content_type_is_json() {
        use axum::response::IntoResponse;

        let err = AppError::Internal("oops".into());
        let response = err.into_response();

        let content_type = response.headers().get("content-type").unwrap();
        assert!(content_type
            .to_str()
            .unwrap()
            .starts_with("application/json"));
    }
}
