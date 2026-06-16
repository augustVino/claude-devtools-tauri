//! TCP pre-probe — distinguish "host unreachable" from "auth rejected"
//! before invoking russh::client::connect.
//!
//! When host is unreachable (firewall, per-app VPN split-tunnel blocking),
//! russh's connect either hangs for the full CONNECT_TIMEOUT or returns
//! a generic error. This probe gives a fast, diagnostic-rich failure.

use std::time::{Duration, Instant};

/// TCP probe timeout — must be < CONNECT_TIMEOUT (10s) to leave room for
/// SSH handshake. 5s aligns with Electron's TcpProbe timeout.
const TCP_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Result of TCP pre-probe.
#[derive(Debug, Clone)]
pub(super) enum TcpProbeResult {
    /// Host:port accepts TCP connections. Proceed to SSH handshake.
    Reachable { elapsed: Duration },
    /// TCP connect returned an error (connection refused, host not found, etc).
    /// `error` is the std::io::Error display string.
    Unreachable { error: String, elapsed: Duration },
    /// TCP connect did not complete within TCP_PROBE_TIMEOUT.
    /// Common cause: firewall dropping packets silently, per-app VPN.
    Timeout { elapsed: Duration },
}

impl TcpProbeResult {
    /// True if probe succeeded (caller can proceed to SSH handshake).
    pub(super) fn is_reachable(&self) -> bool {
        matches!(self, TcpProbeResult::Reachable { .. })
    }

    /// Render a diagnostic message for Unreachable/Timeout. Returns None for Reachable.
    /// Message is multi-line for `whitespace-pre-line` frontend rendering.
    pub(super) fn diagnostic_message(&self, host: &str, port: u16) -> Option<String> {
        match self {
            TcpProbeResult::Reachable { .. } => None,
            TcpProbeResult::Unreachable { error, elapsed } => Some(format!(
                "Host {}:{} unreachable (elapsed: {:.1}s).\n\
                 This app may be blocked by per-app VPN:\n\
                 \x20\x20• Cisco AnyConnect / GlobalProtect / Cloudflare WARP in split-tunnel mode\n\
                 \x20\x20  commonly allow-list only specific apps; add this app or switch to full-tunnel.\n\
                 Original error: {}",
                host, port, elapsed.as_secs_f64(), error
            )),
            TcpProbeResult::Timeout { elapsed } => Some(format!(
                "Host {}:{} did not respond within {:.0}s (TCP probe timeout).\n\
                 Common causes:\n\
                 \x20\x20• Firewall dropping packets silently (no TCP RST returned)\n\
                 \x20\x20• Per-app VPN split-tunnel not routing this destination\n\
                 \x20\x20• Host is up but port closed and not sending RST\n\
                 Elapsed: {:.1}s",
                host, port, TCP_PROBE_TIMEOUT.as_secs(), elapsed.as_secs_f64()
            )),
        }
    }
}

/// Probe TCP reachability of host:port.
///
/// Uses raw `tokio::net::TcpStream::connect` with `TCP_PROBE_TIMEOUT`.
/// Does NOT perform SSH handshake — just checks if TCP layer is responsive.
pub(super) async fn probe_tcp(host: &str, port: u16) -> TcpProbeResult {
    let start = Instant::now();
    let addr = (host, port);

    match tokio::time::timeout(TCP_PROBE_TIMEOUT, tokio::net::TcpStream::connect(addr)).await {
        Ok(Ok(_stream)) => {
            let elapsed = start.elapsed();
            log::debug!(
                "TCP probe to {}:{} succeeded in {:.3}s",
                host, port, elapsed.as_secs_f64()
            );
            TcpProbeResult::Reachable { elapsed }
        }
        Ok(Err(e)) => {
            let elapsed = start.elapsed();
            log::debug!(
                "TCP probe to {}:{} failed in {:.3}s: {}",
                host, port, elapsed.as_secs_f64(), e
            );
            TcpProbeResult::Unreachable {
                error: e.to_string(),
                elapsed,
            }
        }
        Err(_) => {
            let elapsed = start.elapsed();
            log::debug!(
                "TCP probe to {}:{} timed out after {:.0}s",
                host, port, TCP_PROBE_TIMEOUT.as_secs()
            );
            TcpProbeResult::Timeout { elapsed }
        }
    }
}

/// Elapsed time helper — exposes probe duration for AuthTrace collection.
impl TcpProbeResult {
    pub(super) fn elapsed_ms(&self) -> u64 {
        match self {
            TcpProbeResult::Reachable { elapsed }
            | TcpProbeResult::Unreachable { elapsed, .. }
            | TcpProbeResult::Timeout { elapsed } => elapsed.as_millis() as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tcp_probe_result_is_reachable() {
        let r = TcpProbeResult::Reachable { elapsed: Duration::from_millis(50) };
        assert!(r.is_reachable());
        assert_eq!(r.elapsed_ms(), 50);
    }

    #[test]
    fn test_tcp_probe_result_unreachable_no_is_reachable() {
        let r = TcpProbeResult::Unreachable {
            error: "connection refused".to_string(),
            elapsed: Duration::from_millis(10),
        };
        assert!(!r.is_reachable());
        assert_eq!(r.elapsed_ms(), 10);
    }

    #[test]
    fn test_tcp_probe_result_timeout_no_is_reachable() {
        let r = TcpProbeResult::Timeout { elapsed: TCP_PROBE_TIMEOUT };
        assert!(!r.is_reachable());
        assert_eq!(r.elapsed_ms(), TCP_PROBE_TIMEOUT.as_millis() as u64);
    }

    #[test]
    fn test_diagnostic_message_unreachable_includes_vpn_hint() {
        let r = TcpProbeResult::Unreachable {
            error: "connection refused".to_string(),
            elapsed: Duration::from_millis(10),
        };
        let msg = r.diagnostic_message("example.com", 22).unwrap();
        assert!(msg.contains("example.com:22"));
        assert!(msg.contains("per-app VPN"));
        assert!(msg.contains("Cisco AnyConnect"));
        assert!(msg.contains("connection refused"));
    }

    #[test]
    fn test_diagnostic_message_timeout_includes_firewall_hint() {
        let r = TcpProbeResult::Timeout { elapsed: TCP_PROBE_TIMEOUT };
        let msg = r.diagnostic_message("corp.example.com", 2222).unwrap();
        assert!(msg.contains("corp.example.com:2222"));
        assert!(msg.contains("Firewall"));
        // Timeout 变体的源代码用大写 "Per-app VPN"（line 51），Unreachable 用小写 "per-app VPN"（line 41）。
        // 大小写敏感：测试断言要匹配各自变体。
        assert!(msg.contains("Per-app VPN"));
    }

    #[test]
    fn test_diagnostic_message_reachable_returns_none() {
        let r = TcpProbeResult::Reachable { elapsed: Duration::from_millis(5) };
        assert!(r.diagnostic_message("h", 22).is_none());
    }

    /// Real probe test — verifies probe actually runs against a closed port.
    /// Uses port 1 (typically unused, returns RST quickly on Unix).
    /// May be flaky on some CI environments; treated as smoke test.
    #[tokio::test]
    async fn test_probe_tcp_closed_port_returns_unreachable_or_timeout() {
        // Port 1 is reserved and typically closed. Result varies by OS:
        // - macOS/Linux: usually RST → Unreachable
        // - Some CI sandboxes: silent drop → Timeout
        // Both are valid non-Reachable outcomes.
        let result = probe_tcp("127.0.0.1", 1).await;
        assert!(!result.is_reachable(), "Port 1 should not be reachable");
    }

    /// Real Reachable test — bind a temporary TcpListener, probe it,
    /// verify Reachable + elapsed_ms > 0.
    #[tokio::test]
    async fn test_probe_tcp_open_port_returns_reachable() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // probe_tcp 签名是 (host: &str, port: u16)，需要把 IpAddr 转 String。
        let host = addr.ip().to_string();
        let result = probe_tcp(&host, addr.port()).await;
        assert!(result.is_reachable(), "Open port should be reachable");
        drop(listener); // cleanup
    }

    /// Invalid host input — should NOT panic. Returns Unreachable (DNS fail
    /// surfaces as io::Error in TcpStream::connect).
    #[tokio::test]
    async fn test_probe_tcp_invalid_host_does_not_panic() {
        let result = probe_tcp("invalid!host", 0).await;
        // Either Unreachable (DNS resolution error) or Timeout (unlikely but possible).
        // Critical: no panic.
        assert!(!result.is_reachable());
    }
}
