//! 分页游标编解码工具。
//!
//! 对齐 Electron 的 base64(JSON{timestamp, sessionId}) 游标格式，
//! 同时向后兼容纯 sessionId 格式。

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use crate::types::domain::SessionCursor;

/// Encode a cursor from session data (aligned with Electron's base64 JSON format).
pub fn encode_cursor(timestamp: u64, session_id: &str) -> String {
    let data = SessionCursor {
        timestamp,
        session_id: session_id.to_string(),
    };
    let json = serde_json::to_string(&data).unwrap_or_default();
    BASE64.encode(json.as_bytes())
}

/// Decode a cursor. Returns (timestamp, session_id).
/// Falls back to treating the cursor as a plain session ID for backward compatibility.
pub fn decode_cursor(cursor: &str) -> (Option<u64>, String) {
    if let Ok(bytes) = BASE64.decode(cursor) {
        if let Ok(data) = serde_json::from_slice::<SessionCursor>(&bytes) {
            return (Some(data.timestamp), data.session_id);
        }
    }
    // Backward compat: treat as plain session ID
    (None, cursor.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_roundtrip() {
        let encoded = encode_cursor(1234567890, "session-abc");
        let (ts, id) = decode_cursor(&encoded);
        assert_eq!(ts, Some(1234567890));
        assert_eq!(id, "session-abc");
    }

    #[test]
    fn test_decode_plain_session_id_fallback() {
        let (ts, id) = decode_cursor("plain-session-id");
        assert_eq!(ts, None);
        assert_eq!(id, "plain-session-id");
    }
}
