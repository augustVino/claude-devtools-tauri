//! Auth attempt tracing — structured error enrichment for SSH auth failures.
//!
//! Phase 2's `do_auth_agent_multi` accumulated failures as `Vec<String>`.
//! Phase 3a upgrades to structured `AuthAttempt` list + `AuthTimings`,
//! rendered by `enrich_auth_error` into a multi-section diagnostic message.

use std::fmt::Write;

/// Outcome of a single auth attempt (key file or agent candidate).
#[derive(Debug, Clone)]
pub(crate) enum AttemptOutcome {
    /// Auth succeeded with this candidate.
    Used,
    /// Candidate was skipped (e.g., key file not found, encrypted key unsupported).
    Skipped { reason: String },
    /// Candidate was tried but rejected by server or encountered error.
    Failed { reason: String },
}

/// A single auth attempt with source label + outcome.
#[derive(Debug, Clone)]
pub(crate) struct AuthAttempt {
    /// Human-readable source: "IdentityAgent ~/Library/..." / "~/.ssh/id_ed25519" / ...
    /// Pre-masked by caller (mask_home_path) to avoid leaking $HOME.
    pub source: String,
    pub outcome: AttemptOutcome,
}

/// Per-stage timings collected during establish_raw_connection.
#[derive(Debug, Clone, Default)]
pub(crate) struct AuthTimings {
    /// ssh -G resolution elapsed (ms). 0 if fallback was used immediately.
    pub resolve_ms: u64,
    /// TCP pre-probe elapsed (ms). 0 if probe was skipped (error before probe).
    pub tcp_probe_ms: u64,
    /// russh::client::connect elapsed (ms). None if error occurred before
    /// handshake completed (in-flight).
    pub tcp_handshake_ms: Option<u64>,
    /// Per-attempt durations: (source, ms). Empty if no attempts were made.
    pub auth_attempts_ms: Vec<(String, u64)>,
}

/// Full trace collected during a connection attempt.
#[derive(Debug, Clone, Default)]
pub(crate) struct AuthTrace {
    pub attempts: Vec<AuthAttempt>,
    pub timings: AuthTimings,
}

impl AuthTrace {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Append a single attempt with timing.
    pub(crate) fn record_attempt(&mut self, source: impl Into<String>, outcome: AttemptOutcome, ms: u64) {
        let source = source.into();
        self.attempts.push(AuthAttempt {
            source: source.clone(),
            outcome,
        });
        self.timings.auth_attempts_ms.push((source, ms));
    }
}

/// Render an auth error message with trace context (multi-section).
///
/// If trace is empty (no attempts/timings), returns just `root_message`.
pub(crate) fn enrich_auth_error(root_message: &str, trace: &AuthTrace) -> String {
    let mut sections: Vec<String> = vec![root_message.to_string()];

    // Auth chain section
    if !trace.attempts.is_empty() {
        let mut chain = String::from("Auth chain:");
        for attempt in &trace.attempts {
            let outcome_str = match &attempt.outcome {
                AttemptOutcome::Used => "used".to_string(),
                AttemptOutcome::Skipped { reason } => format!("skipped ({})", reason),
                AttemptOutcome::Failed { reason } => format!("failed ({})", reason),
            };
            // \x20\x20 = 两个空格（bullet 缩进）。Rust 字符串字面量单反斜杠。
            write!(chain, "\n\x20\x20• {} — {}", attempt.source, outcome_str).unwrap();
        }
        sections.push(chain);
    }

    // Timing section
    let has_timings = trace.timings.resolve_ms > 0
        || trace.timings.tcp_probe_ms > 0
        || trace.timings.tcp_handshake_ms.is_some()
        || !trace.timings.auth_attempts_ms.is_empty();
    if has_timings {
        let mut timing = String::from("Timing:");
        if trace.timings.resolve_ms > 0 {
            write!(timing, "\n\x20\x20• resolve: {}ms", trace.timings.resolve_ms).unwrap();
        }
        if trace.timings.tcp_probe_ms > 0 {
            write!(timing, "\n\x20\x20• tcp probe: {}ms", trace.timings.tcp_probe_ms).unwrap();
        }
        match trace.timings.tcp_handshake_ms {
            Some(ms) => write!(timing, "\n\x20\x20• tcp+handshake: {}ms", ms).unwrap(),
            None => write!(timing, "\n\x20\x20• tcp+handshake: (in-flight)").unwrap(),
        }
        if !trace.timings.auth_attempts_ms.is_empty() {
            let attempts_str: Vec<String> = trace
                .timings
                .auth_attempts_ms
                .iter()
                .map(|(s, ms)| format!("{}={}ms", s, ms))
                .collect();
            write!(timing, "\n\x20\x20• attempts: {}", attempts_str.join(", ")).unwrap();
        }
        sections.push(timing);
    }

    sections.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enrich_empty_trace_returns_root_only() {
        let trace = AuthTrace::new();
        let msg = enrich_auth_error("auth failed", &trace);
        assert_eq!(msg, "auth failed");
    }

    #[test]
    fn test_enrich_includes_auth_chain_section() {
        let mut trace = AuthTrace::new();
        trace.record_attempt(
            "~/.ssh/id_ed25519",
            AttemptOutcome::Failed {
                reason: "rejected by server".to_string(),
            },
            120,
        );
        trace.record_attempt(
            "IdentityAgent (1Password)",
            AttemptOutcome::Used,
            350,
        );

        let msg = enrich_auth_error("Auto auth failed", &trace);
        assert!(msg.contains("Auto auth failed"));
        assert!(msg.contains("Auth chain:"));
        assert!(msg.contains("~/.ssh/id_ed25519 — failed (rejected by server)"));
        assert!(msg.contains("IdentityAgent (1Password) — used"));
    }

    #[test]
    fn test_enrich_includes_skipped_outcome() {
        let mut trace = AuthTrace::new();
        trace.record_attempt(
            "~/.ssh/id_rsa",
            AttemptOutcome::Skipped {
                reason: "encrypted; passphrase not supported".to_string(),
            },
            5,
        );

        let msg = enrich_auth_error("auth failed", &trace);
        assert!(msg.contains("~/.ssh/id_rsa — skipped (encrypted; passphrase not supported)"));
    }

    #[test]
    fn test_enrich_includes_timing_section() {
        let mut trace = AuthTrace::new();
        trace.timings.resolve_ms = 12;
        trace.timings.tcp_probe_ms = 4;
        trace.timings.tcp_handshake_ms = Some(2301);
        trace.record_attempt("id_ed25519", AttemptOutcome::Used, 120);

        let msg = enrich_auth_error("auth failed", &trace);
        assert!(msg.contains("Timing:"));
        assert!(msg.contains("resolve: 12ms"));
        assert!(msg.contains("tcp probe: 4ms"));
        assert!(msg.contains("tcp+handshake: 2301ms"));
        assert!(msg.contains("attempts: id_ed25519=120ms"));
    }

    #[test]
    fn test_enrich_handshake_in_flight_shown_when_none() {
        let mut trace = AuthTrace::new();
        trace.timings.resolve_ms = 10;
        // tcp_handshake_ms stays None (error before handshake completed)
        trace.record_attempt("k", AttemptOutcome::Used, 1);

        let msg = enrich_auth_error("early failure", &trace);
        assert!(msg.contains("tcp+handshake: (in-flight)"));
    }

    #[test]
    fn test_enrich_multi_section_separator() {
        // Verify sections are separated by exactly \n\n (markdown paragraph break,
        // rendered as blank line by whitespace-pre-line).
        let mut trace = AuthTrace::new();
        trace.record_attempt("a", AttemptOutcome::Used, 1);
        trace.timings.resolve_ms = 1;

        let msg = enrich_auth_error("root", &trace);
        assert!(msg.contains("root\n\nAuth chain:"));
        // \x20\x20 = 两空格（bullet 缩进）。Rust 字符串字面量。
        assert!(msg.contains("Auth chain:\n\x20\x20• a — used\n\nTiming:"));
        // Verify multi-line structure: count newlines to detect accidental flattening.
        // 实际结构：root\n\nAuth chain:\n  • a — used\n\nTiming:\n  • resolve: 1ms\n  • tcp+handshake: (in-flight)\n  • attempts: a=1ms
        // newlines: 2 (sections.join root→Auth chain) + 1 (Auth chain header→bullet) +
        // 2 (sections.join Auth chain→Timing) + 1 (Timing header→resolve bullet) +
        // 1 (resolve→tcp+handshake bullet) + 1 (tcp+handshake→attempts bullet) = 8
        // ⚠️ record_attempt 触发 attempts 行；tcp_handshake_ms=None 始终渲染 (in-flight) 行；
        // resolve_ms=1 触发 resolve 行。原 plan 算 7 是漏了 (in-flight) 行。
        assert_eq!(msg.matches('\n').count(), 8, "Expected 8 newlines for multi-section format");
    }

    #[test]
    fn test_record_attempt_appends_to_both_attempts_and_timings() {
        let mut trace = AuthTrace::new();
        trace.record_attempt(
            "src1",
            AttemptOutcome::Failed { reason: "x".into() },
            100,
        );
        assert_eq!(trace.attempts.len(), 1);
        assert_eq!(trace.timings.auth_attempts_ms.len(), 1);
        assert_eq!(trace.timings.auth_attempts_ms[0], ("src1".to_string(), 100));
    }
}
