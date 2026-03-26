//! 语义步骤的时间线间隙填充模块。
//!
//! 扩展步骤的 `effective_end_time`，使可视化时间线在连续操作之间没有
//! 死区间隙。持续时间超过 `MEANINGFUL_SUBAGENT_DURATION_MS` 的子 Agent
//! 保留其原始时间，因为它们代表真实的、可测量的工作。

use crate::types::messages::{SemanticStep, SemanticStepType};

/// 步骤之间的启动间隔在此窗口（毫秒）内视为并行执行。
const PARALLEL_WINDOW_MS: u64 = 100;

/// 子 Agent 原始持续时间超过此阈值时保留真实时间，不进行间隙填充。
const MEANINGFUL_SUBAGENT_DURATION_MS: u64 = 100;

/// [`fill_timeline_gaps`] 的输入参数包。
pub struct GapFillingInput<'a> {
    /// 待原地修改的语义步骤切片。
    pub steps: &'a mut [SemanticStep],
    /// 块起始时间（epoch 毫秒）。
    pub chunk_start_time_ms: u64,
    /// 块结束时间（epoch 毫秒）。
    pub chunk_end_time_ms: u64,
}

/// 填充连续语义步骤之间的时间线间隙。
///
/// 对每个步骤（持续时间有意义的子 Agent 除外），当步骤间间隙超过
/// `PARALLEL_WINDOW_MS` 时，将 `effective_end_time` 延伸到下一个步骤的
/// 起始时间。最后一个步骤延伸到 `chunk_end_time_ms`。
pub fn fill_timeline_gaps(input: GapFillingInput<'_>) {
    let steps = input.steps;
    if steps.is_empty() {
        return;
    }

    let chunk_end = input.chunk_end_time_ms;

    for i in 0..steps.len() {
        let step = &steps[i];

        // 持续时间有意义的子 Agent 保留其原始时间
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
                // 并行 — 保持原始时长，不延伸
                current_start_ms + step.duration_ms
            } else {
                // 有间隙 — 延伸到下一个步骤的起始时间
                next_start_ms
            }
        } else {
            // 最后一个步骤 — 延伸到块结束时间
            chunk_end
        };

        let effective_duration = effective_end.saturating_sub(current_start_ms);

        steps[i].effective_end_time = Some(format_ts_ms(effective_end));
        steps[i].effective_duration_ms = Some(effective_duration);
        steps[i].is_gap_filled = Some(true);
    }
}

/// 将 RFC 3339 时间戳字符串解析为 epoch 毫秒。
pub fn parse_ts_ms(ts: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.timestamp_millis() as u64)
}

/// 将 epoch 毫秒格式化为 RFC 3339 时间戳字符串。
pub fn format_ts_ms(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

// =============================================================================
// 单元测试
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

    // -- 时间戳辅助函数 --------------------------------------------------------

    #[test]
    fn parse_ts_ms_valid() {
        let ms = parse_ts_ms("2026-03-25T10:00:00.000Z").unwrap();
        // 验证值通过 格式化 -> 解析 的往返一致性
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

    // -- 间隙填充 --------------------------------------------------------------

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

        // s1: time_diff = 50ms <= 100ms => 并行，保持原始 50ms
        assert_eq!(steps[0].effective_duration_ms, Some(50));

        // s2: 最后一个步骤，延伸到块结束时间
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

        // s1: time_diff = 1000ms > 100ms => 延伸到下一个步骤起始
        assert_eq!(steps[0].effective_duration_ms, Some(1000));

        // s2: 最后一个步骤，延伸到块结束时间
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

        // 持续时间 > 100ms 的子 Agent 保留原始 5000ms
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

        // 持续时间 <= 100ms 的子 Agent 进行间隙填充到块结束
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
