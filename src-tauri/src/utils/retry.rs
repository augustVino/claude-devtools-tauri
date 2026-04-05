//! 通用瞬态错误同步重试，带指数退避。
//!
//! 用于 SshFsProvider 中 SFTP 操作的重试逻辑提取。
//! 注意：SshFsProvider 通过 block_in_place 将异步 SFTP 调用桥接为同步，
//! 因此 retry 本身是同步函数（不涉及 async await）。

use std::path::Path;
use std::thread;
use std::time::Duration;

/// 默认最大重试次数（不含首次尝试）
#[allow(dead_code)]
pub const DEFAULT_MAX_RETRIES: u32 = 3;
/// 默认首次重试延迟（毫秒），线性递增
#[allow(dead_code)]
pub const DEFAULT_BASE_DELAY_MS: u64 = 75;

/// 执行带瞬态错误重试的操作。
///
/// # 参数
/// - `op_name`: 操作名称（用于日志前缀，如 "SFTP read_file"）
/// - `path`: 操作路径（用于日志）
/// - `operation`: 要执行的操作闭包
/// - `max_retries`: 最大重试次数
/// - `base_delay_ms`: 基础延迟毫秒数（每次重试线性递增：base * attempt）
/// - `is_transient`: 判断错误是否为瞬态（可重试）的谓词
///
/// # 返回
/// - `Ok(T)` — 操作成功或 NotFound 类错误被正确处理
/// - `Err(E)` — 永久性错误或重试耗尽后的最后一个错误
pub fn retry_transient<T, E, F>(
    op_name: &str,
    path: &Path,
    mut operation: F,
    max_retries: u32,
    base_delay_ms: u64,
    is_transient: &dyn Fn(&E) -> bool,
) -> Result<T, E>
where
    F: FnMut() -> Result<T, E>,
{
    let mut last_err = None;

    for attempt in 0..=max_retries {
        match operation() {
            Ok(result) => return Ok(result),
            Err(err) => {
                if !is_transient(&err) {
                    return Err(err);
                } else if attempt < max_retries {
                    let delay = base_delay_ms * (attempt as u64 + 1);
                    log::warn!(
                        "{} transient error (attempt {}/{}) for {:?}, retrying in {}ms",
                        op_name,
                        attempt + 1,
                        max_retries + 1,
                        path,
                        delay
                    );
                    thread::sleep(Duration::from_millis(delay));
                    last_err = Some(err);
                } else {
                    return Err(err);
                }
            }
        }
    }

    Err(last_err.unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    enum TestError {
        Transient(u32),
        Permanent,
    }

    /// 瞬态错误：前 N 次失败，第 N+1 次成功
    fn failing_op(fail_count: &mut u32) -> Result<(), TestError> {
        if *fail_count > 0 {
            *fail_count -= 1;
            Err(TestError::Transient(*fail_count))
        } else {
            Ok(())
        }
    }

    #[test]
    fn test_retry_succeeds_on_second_try() {
        let mut fails = 1;
        let result = retry_transient(
            "test_op",
            Path::new("/tmp/test"),
            || { failing_op(&mut fails) },
            3,
            10,
            &|e| matches!(e, TestError::Transient(_)),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_retry_exhausts_and_returns_last_error() {
        let mut fails = 10; // more than max_retries
        let result = retry_transient(
            "test_op",
            Path::new("/tmp/test"),
            || { failing_op(&mut fails) },
            3,
            1, // minimal delay for tests
            &|e| matches!(e, TestError::Transient(_)),
        );
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), TestError::Transient(6)); // 10 - 3 - 1
    }

    #[test]
    fn test_permanent_error_no_retry() {
        let call_count = std::sync::atomic::AtomicU32::new(0);
        let result: Result<(), TestError> = retry_transient(
            "test_op",
            Path::new("/tmp/test"),
            || {
                call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(TestError::Permanent)
            },
            3,
            1,
            &|e| matches!(e, TestError::Transient(_)),
        );
        assert!(result.is_err());
        assert_eq!(
            call_count.load(std::sync::atomic::Ordering::SeqCst),
            1
        ); // only called once
    }
}
