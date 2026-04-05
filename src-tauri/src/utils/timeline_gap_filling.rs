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
    #[allow(dead_code)]
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
                .unwrap_or(step.start_time + step.duration_ms);

            let start_ms = step.start_time;
            let effective_duration = effective_end.saturating_sub(start_ms);

            steps[i].effective_end_time = Some(effective_end);
            steps[i].effective_duration_ms = Some(effective_duration);
            steps[i].is_gap_filled = Some(true);
            continue;
        }

        let current_start_ms = step.start_time;

        let effective_end = if i + 1 < steps.len() {
            let next_start_ms = steps[i + 1].start_time;
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

        steps[i].effective_end_time = Some(effective_end);
        steps[i].effective_duration_ms = Some(effective_duration);
        steps[i].is_gap_filled = Some(true);
    }
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
        start_time: u64,
        end_time: Option<u64>,
        duration_ms: u64,
        step_type: SemanticStepType,
    ) -> SemanticStep {
        SemanticStep {
            id: id.to_string(),
            step_type,
            start_time,
            end_time,
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

    /// Helper: parse ISO timestamp to u64 ms, panics on failure.
    fn ts(s: &str) -> u64 {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.timestamp_millis() as u64)
            .expect("valid timestamp")
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
        let start = ts("2026-03-25T10:00:00.000Z");
        let chunk_end = ts("2026-03-25T10:00:05.000Z");

        let mut steps = vec![make_step(
            "s1",
            start,
            None,
            50,
            SemanticStepType::Thinking,
        )];

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
            steps[0].effective_end_time,
            Some(chunk_end)
        );
    }

    #[test]
    fn fill_gaps_parallel_steps_no_extension() {
        let t0 = ts("2026-03-25T10:00:00.000Z");
        let t1 = ts("2026-03-25T10:00:00.050Z");
        let chunk_end = ts("2026-03-25T10:00:10.000Z");

        let mut steps = vec![
            make_step("s1", t0, None, 50, SemanticStepType::Thinking),
            make_step("s2", t1, None, 30, SemanticStepType::Output),
        ];

        fill_timeline_gaps(GapFillingInput {
            steps: &mut steps,
            chunk_start_time_ms: chunk_end - 10000,
            chunk_end_time_ms: chunk_end,
        });

        // s1: time_diff = 50ms <= 100ms => parallel, keep original 50ms
        assert_eq!(steps[0].effective_duration_ms, Some(50));

        // s2: last step, extend to chunk end
        assert_eq!(steps[1].effective_duration_ms, Some(9950));
    }

    #[test]
    fn fill_gaps_gap_between_steps() {
        let t0 = ts("2026-03-25T10:00:00.000Z");
        let t1 = ts("2026-03-25T10:00:01.000Z");
        let chunk_end = ts("2026-03-25T10:00:10.000Z");

        let mut steps = vec![
            make_step("s1", t0, None, 50, SemanticStepType::Thinking),
            make_step("s2", t1, None, 30, SemanticStepType::ToolCall),
        ];

        fill_timeline_gaps(GapFillingInput {
            steps: &mut steps,
            chunk_start_time_ms: chunk_end - 10000,
            chunk_end_time_ms: chunk_end,
        });

        // s1: time_diff = 1000ms > 100ms => extend to next step start
        assert_eq!(steps[0].effective_duration_ms, Some(1000));

        // s2: last step, extend to chunk end
        assert_eq!(steps[1].effective_duration_ms, Some(9000));
    }

    #[test]
    fn fill_gaps_meaningful_subagent_keeps_original_timing() {
        let start = ts("2026-03-25T10:00:00.000Z");
        let end = ts("2026-03-25T10:00:05.000Z");
        let chunk_end = ts("2026-03-25T10:00:10.000Z");

        let mut steps = vec![make_step(
            "sub-1",
            start,
            Some(end),
            5000,
            SemanticStepType::Subagent,
        )];

        fill_timeline_gaps(GapFillingInput {
            steps: &mut steps,
            chunk_start_time_ms: chunk_end - 10000,
            chunk_end_time_ms: chunk_end,
        });

        // Subagent with duration > 100ms keeps original 5000ms
        assert_eq!(steps[0].effective_duration_ms, Some(5000));
        assert_eq!(steps[0].effective_end_time, Some(end));
    }

    #[test]
    fn fill_gaps_short_subagent_gets_gap_filled() {
        let start = ts("2026-03-25T10:00:00.000Z");
        let end = ts("2026-03-25T10:00:00.050Z");
        let chunk_end = ts("2026-03-25T10:00:05.000Z");

        let mut steps = vec![make_step(
            "sub-short",
            start,
            Some(end),
            50,
            SemanticStepType::Subagent,
        )];

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
        let start = ts("2026-03-25T10:00:00.000Z");

        let mut steps = vec![make_step(
            "s1",
            start,
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
