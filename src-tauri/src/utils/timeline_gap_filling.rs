//! Timeline gap filling for semantic steps.
//!
//! Extends `effective_end_time` on steps so that the visual timeline has no
//! dead gaps between consecutive operations.  Subagents with duration above
//! `MEANINGFUL_SUBAGENT_DURATION_MS` keep their original timing since they
//! represent real, measurable work.

use crate::types::messages::{SemanticStep, SemanticStepType};

/// Steps starting within this window (ms) of each other are considered parallel.
const PARALLEL_WINDOW_MS: u64 = 100;

/// Subagents whose original duration exceeds this threshold retain their real
/// timing instead of being gap-filled.
const MEANINGFUL_SUBAGENT_DURATION_MS: u64 = 100;

/// Input bundle for [`fill_timeline_gaps`].
pub struct GapFillingInput<'a> {
    /// The semantic steps to mutate in-place.
    pub steps: &'a mut [SemanticStep],
    /// Chunk start time in epoch milliseconds.
    pub chunk_start_time_ms: u64,
    /// Chunk end time in epoch milliseconds.
    pub chunk_end_time_ms: u64,
}

/// Fill timeline gaps between consecutive semantic steps.
///
/// For each step (except subagents with meaningful duration), the
/// `effective_end_time` is extended to the start time of the next step
/// when the inter-step gap exceeds `PARALLEL_WINDOW_MS`.  The last step
/// is extended to `chunk_end_time_ms`.
pub fn fill_timeline_gaps(input: GapFillingInput<'_>) {
    let steps = input.steps;
    if steps.is_empty() {
        return;
    }

    let chunk_end = input.chunk_end_time_ms;

    for i in 0..steps.len() {
        let step = &steps[i];

        // Subagents with meaningful duration keep their original timing.
        if step.step_type == SemanticStepType::Subagent
            && step.duration_ms > MEANINGFUL_SUBAGENT_DURATION_MS
        {
            let effective_end = step
                .end_time
                .as_deref()
                .and_then(parse_ts_ms)
                .unwrap_or_else(|| {
                    parse_ts_ms(&step.start_time).unwrap_or(0) + step.duration_ms
                });

            let start_ms = parse_ts_ms(&step.start_time).unwrap_or(0);
            let effective_duration = effective_end.saturating_sub(start_ms);

            steps[i].effective_end_time = Some(format_ts_ms(effective_end));
            steps[i].effective_duration_ms = Some(effective_duration);
            steps[i].is_gap_filled = Some(true);
            continue;
        }

        let current_start_ms = parse_ts_ms(&step.start_time).unwrap_or(0);

        let effective_end = if i + 1 < steps.len() {
            let next_start_ms = parse_ts_ms(&steps[i + 1].start_time).unwrap_or(0);
            let time_diff = next_start_ms.saturating_sub(current_start_ms);

            if time_diff <= PARALLEL_WINDOW_MS {
                // Parallel — keep original duration, don't extend.
                current_start_ms + step.duration_ms
            } else {
                // Gap — extend to the next step's start.
                next_start_ms
            }
        } else {
            // Last step — extend to chunk end.
            chunk_end
        };

        let effective_duration = effective_end.saturating_sub(current_start_ms);

        steps[i].effective_end_time = Some(format_ts_ms(effective_end));
        steps[i].effective_duration_ms = Some(effective_duration);
        steps[i].is_gap_filled = Some(true);
    }
}

/// Parse an RFC 3339 timestamp string into epoch milliseconds.
pub fn parse_ts_ms(ts: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.timestamp_millis() as u64)
}

/// Format epoch milliseconds as an RFC 3339 timestamp string.
pub fn format_ts_ms(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::messages::{SemanticStepContent, StepTokens};

    fn make_step(
        id: &str,
        start_time: &str,
        end_time: Option<&str>,
        duration_ms: u64,
        step_type: SemanticStepType,
    ) -> SemanticStep {
        SemanticStep {
            id: id.to_string(),
            step_type,
            start_time: start_time.to_string(),
            end_time: end_time.map(|s| s.to_string()),
            duration_ms,
            content: SemanticStepContent::default(),
            tokens: Some(StepTokens {
                input: 0,
                output: 0,
                cached: None,
            }),
            is_parallel: None,
            group_id: None,
            context: "main".to_string(),
            agent_id: None,
            source_message_id: None,
            effective_end_time: None,
            effective_duration_ms: None,
            is_gap_filled: None,
            context_tokens: None,
            accumulated_context: None,
            token_breakdown: None,
        }
    }

    // -- Timestamp helpers ----------------------------------------------------

    #[test]
    fn parse_ts_ms_valid() {
        let ms = parse_ts_ms("2026-03-25T10:00:00.000Z").unwrap();
        // Verify the value round-trips through format -> parse.
        let formatted = format_ts_ms(ms);
        let roundtrip = parse_ts_ms(&formatted).unwrap();
        assert_eq!(ms, roundtrip);
    }

    #[test]
    fn parse_ts_ms_invalid_returns_none() {
        assert!(parse_ts_ms("not-a-date").is_none());
        assert!(parse_ts_ms("").is_none());
    }

    #[test]
    fn format_ts_ms_roundtrip() {
        let original = "2026-03-25T10:00:00.000+00:00";
        let ms = parse_ts_ms(original).unwrap();
        let formatted = format_ts_ms(ms);
        let roundtrip = parse_ts_ms(&formatted).unwrap();
        assert_eq!(ms, roundtrip);
    }

    #[test]
    fn format_ts_ms_zero_returns_epoch() {
        let result = format_ts_ms(0);
        assert!(!result.is_empty(), "Epoch zero should produce a valid timestamp");
        assert!(result.starts_with("1970-01-01"));
    }

    // -- Gap filling -----------------------------------------------------------

    #[test]
    fn fill_gaps_empty_steps() {
        let mut steps: Vec<SemanticStep> = vec![];
        fill_timeline_gaps(GapFillingInput {
            steps: &mut steps,
            chunk_start_time_ms: 0,
            chunk_end_time_ms: 1000,
        });
        assert!(steps.is_empty());
    }

    #[test]
    fn fill_gaps_single_step_extends_to_chunk_end() {
        let mut steps = vec![make_step(
            "s1",
            "2026-03-25T10:00:00.000Z",
            None,
            50,
            SemanticStepType::Thinking,
        )];

        let chunk_end = parse_ts_ms("2026-03-25T10:00:05.000Z").unwrap();

        fill_timeline_gaps(GapFillingInput {
            steps: &mut steps,
            chunk_start_time_ms: chunk_end - 5000,
            chunk_end_time_ms: chunk_end,
        });

        assert_eq!(steps[0].is_gap_filled, Some(true));
        assert_eq!(
            steps[0].effective_duration_ms,
            Some(5000),
            "Single step should extend to chunk end (5000ms)"
        );
        assert_eq!(
            steps[0].effective_end_time.as_deref(),
            Some("2026-03-25T10:00:05+00:00")
        );
    }

    #[test]
    fn fill_gaps_parallel_steps_no_extension() {
        let mut steps = vec![
            make_step(
                "s1",
                "2026-03-25T10:00:00.000Z",
                None,
                50,
                SemanticStepType::Thinking,
            ),
            make_step(
                "s2",
                "2026-03-25T10:00:00.050Z",
                None,
                30,
                SemanticStepType::Output,
            ),
        ];

        let chunk_end = parse_ts_ms("2026-03-25T10:00:10.000Z").unwrap();

        fill_timeline_gaps(GapFillingInput {
            steps: &mut steps,
            chunk_start_time_ms: chunk_end - 10000,
            chunk_end_time_ms: chunk_end,
        });

        // s1: time_diff = 50ms <= 100ms => parallel, keep original 50ms
        assert_eq!(steps[0].effective_duration_ms, Some(50));

        // s2: last step, extends to chunk end
        assert_eq!(steps[1].effective_duration_ms, Some(9950));
    }

    #[test]
    fn fill_gaps_gap_between_steps() {
        let mut steps = vec![
            make_step(
                "s1",
                "2026-03-25T10:00:00.000Z",
                None,
                50,
                SemanticStepType::Thinking,
            ),
            make_step(
                "s2",
                "2026-03-25T10:00:01.000Z",
                None,
                30,
                SemanticStepType::ToolCall,
            ),
        ];

        let chunk_end = parse_ts_ms("2026-03-25T10:00:10.000Z").unwrap();

        fill_timeline_gaps(GapFillingInput {
            steps: &mut steps,
            chunk_start_time_ms: chunk_end - 10000,
            chunk_end_time_ms: chunk_end,
        });

        // s1: time_diff = 1000ms > 100ms => extend to next step start
        assert_eq!(steps[0].effective_duration_ms, Some(1000));

        // s2: last step, extends to chunk end
        assert_eq!(steps[1].effective_duration_ms, Some(9000));
    }

    #[test]
    fn fill_gaps_meaningful_subagent_keeps_original_timing() {
        let mut steps = vec![make_step(
            "sub-1",
            "2026-03-25T10:00:00.000Z",
            Some("2026-03-25T10:00:05.000Z"),
            5000,
            SemanticStepType::Subagent,
        )];

        let chunk_end = parse_ts_ms("2026-03-25T10:00:10.000Z").unwrap();

        fill_timeline_gaps(GapFillingInput {
            steps: &mut steps,
            chunk_start_time_ms: chunk_end - 10000,
            chunk_end_time_ms: chunk_end,
        });

        // Subagent with duration > 100ms keeps its original 5000ms
        assert_eq!(steps[0].effective_duration_ms, Some(5000));
        assert_eq!(
            steps[0].effective_end_time.as_deref(),
            Some("2026-03-25T10:00:05+00:00")
        );
    }

    #[test]
    fn fill_gaps_short_subagent_gets_gap_filled() {
        let mut steps = vec![make_step(
            "sub-short",
            "2026-03-25T10:00:00.000Z",
            Some("2026-03-25T10:00:00.050Z"),
            50,
            SemanticStepType::Subagent,
        )];

        let chunk_end = parse_ts_ms("2026-03-25T10:00:05.000Z").unwrap();

        fill_timeline_gaps(GapFillingInput {
            steps: &mut steps,
            chunk_start_time_ms: chunk_end - 5000,
            chunk_end_time_ms: chunk_end,
        });

        // Subagent with duration <= 100ms gets gap-filled to chunk end
        assert_eq!(steps[0].effective_duration_ms, Some(5000));
    }

    #[test]
    fn fill_gaps_all_flags_set() {
        let mut steps = vec![make_step(
            "s1",
            "2026-03-25T10:00:00.000Z",
            None,
            50,
            SemanticStepType::Output,
        )];

        fill_timeline_gaps(GapFillingInput {
            steps: &mut steps,
            chunk_start_time_ms: 0,
            chunk_end_time_ms: 100,
        });

        assert_eq!(steps[0].is_gap_filled, Some(true));
        assert!(steps[0].effective_end_time.is_some());
        assert!(steps[0].effective_duration_ms.is_some());
    }
}
