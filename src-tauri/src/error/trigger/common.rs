//! 辅助函数 —— 内容截断与时间戳解析。

/// 将内容截断到最大长度用于显示。
pub fn truncate_content(content: &str, max_len: usize) -> String {
    if content.len() <= max_len {
        return content.to_string();
    }
    // 尝试在字符边界处截断
    let end = content
        .char_indices()
        .take_while(|(i, _)| *i < max_len)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(max_len.min(content.len()));
    format!("{}...", &content[..end])
}

/// 将时间戳字符串解析为自纪元以来的毫秒数。
///
/// 支持 ISO 8601 格式字符串和毫秒时间戳。
pub fn parse_timestamp_to_ms(timestamp: &str) -> u64 {
    // 首先尝试解析为毫秒时间戳
    if let Ok(ms) = timestamp.parse::<u64>() {
        return ms;
    }

    // 尝试 ISO 8601 解析
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(timestamp) {
        return dt.timestamp_millis() as u64;
    }

    // 回退：返回 0
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_content_short() {
        assert_eq!(truncate_content("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_content_long() {
        let result = truncate_content("a".repeat(100).as_str(), 50);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 53); // 50 chars + "..."
    }

    #[test]
    fn test_parse_timestamp_to_ms_iso() {
        let ms = parse_timestamp_to_ms("2025-01-01T00:00:00.000Z");
        assert!(ms > 0);
        assert_eq!(ms, 1735689600000);
    }

    #[test]
    fn test_parse_timestamp_to_ms_number() {
        let ms = parse_timestamp_to_ms("1700000000000");
        assert_eq!(ms, 1700000000000);
    }

    #[test]
    fn test_parse_timestamp_to_ms_invalid() {
        let ms = parse_timestamp_to_ms("not-a-timestamp");
        assert_eq!(ms, 0);
    }
}
