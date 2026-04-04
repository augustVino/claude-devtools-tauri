//! 时间转换工具函数。

use std::time::SystemTime;

/// 将 `Option<SystemTime>` 转换为毫秒时间戳。
/// 如果时间不可用（如 Linux 上 birthtime），返回 0。
///
/// 统一了 LocalFsProvider（通过 .ok() 得到 Option）和
/// SshFsProvider（通过 .ok() 适配 io::Result）的时间转换逻辑。
pub fn time_to_ms(time: Option<SystemTime>) -> u64 {
    match time {
        Some(t) => t
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_time_to_ms_some() {
        let now = SystemTime::now();
        let ms = time_to_ms(Some(now));
        assert!(ms > 1_700_000_000_000); // sanity: after 2023
    }

    #[test]
    fn test_time_to_ms_none() {
        assert_eq!(time_to_ms(None), 0);
    }
}
