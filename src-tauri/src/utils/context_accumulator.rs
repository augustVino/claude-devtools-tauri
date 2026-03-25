//! Context accumulation for semantic steps.
//!
//! Calculates the `accumulated_context` token count for each step, representing
//! the total context window tokens consumed at that point in the session.
//!
//! - **Subagent steps**: use their own `tokens.input + tokens.cached`.
//! - **Other steps**: look up the source assistant message by `source_message_id`
//!   and sum `input_tokens + cache_read + cache_creation`.

use std::collections::HashMap;

use crate::types::messages::{ParsedMessage, SemanticStep, SemanticStepType};

/// Calculate and set `accumulated_context` on each semantic step.
///
/// Builds a lookup from message UUID to `ParsedMessage`, then for each step:
/// - Subagent steps use their own token data.
/// - All other steps resolve their source message and accumulate from its usage.
pub fn calculate_step_context(steps: &mut [SemanticStep], messages: &[ParsedMessage]) {
    let msg_by_uuid: HashMap<&str, &ParsedMessage> =
        messages.iter().map(|m| (m.uuid.as_str(), m)).collect();

    for step in steps.iter_mut() {
        if step.step_type == SemanticStepType::Subagent {
            if let Some(ref tokens) = step.tokens {
                let cached = tokens.cached.unwrap_or(0);
                step.accumulated_context = Some(tokens.input + cached);
            }
            continue;
        }

        let source_id = match &step.source_message_id {
            Some(id) if !id.is_empty() => id.as_str(),
            _ => continue,
        };

        let msg = match msg_by_uuid.get(source_id) {
            Some(m) => m,
            None => continue,
        };

        let usage = match &msg.usage {
            Some(u) => u,
            None => continue,
        };

        let cache_read = usage.cache_read_input_tokens.unwrap_or(0);
        let cache_creation = usage.cache_creation_input_tokens.unwrap_or(0);
        step.accumulated_context = Some(usage.input_tokens + cache_read + cache_creation);
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::domain::MessageType;
    use crate::types::jsonl::UsageMetadata;
    use crate::types::messages::{SemanticStepContent, StepTokens};

    fn make_step(
        id: &str,
        step_type: SemanticStepType,
        source_message_id: Option<&str>,
        tokens: Option<StepTokens>,
    ) -> SemanticStep {
        SemanticStep {
            id: id.to_string(),
            step_type,
            start_time: String::new(),
            end_time: None,
            duration_ms: 0,
            content: SemanticStepContent::default(),
            tokens,
            is_parallel: None,
            group_id: None,
            context: "main".to_string(),
            agent_id: None,
            source_message_id: source_message_id.map(|s| s.to_string()),
            effective_end_time: None,
            effective_duration_ms: None,
            is_gap_filled: None,
            context_tokens: None,
            accumulated_context: None,
            token_breakdown: None,
        }
    }

    fn make_message(
        uuid: &str,
        usage: Option<UsageMetadata>,
    ) -> ParsedMessage {
        ParsedMessage {
            uuid: uuid.to_string(),
            parent_uuid: None,
            message_type: MessageType::Assistant,
            timestamp: "2026-03-25T10:00:00.000Z".to_string(),
            role: Some("assistant".to_string()),
            content: serde_json::Value::Null,
            usage,
            model: None,
            cwd: None,
            git_branch: None,
            agent_id: None,
            is_sidechain: false,
            is_meta: false,
            user_type: None,
            tool_calls: vec![],
            tool_results: vec![],
            source_tool_use_id: None,
            source_tool_assistant_uuid: None,
            tool_use_result: None,
            is_compact_summary: None,
            request_id: None,
        }
    }

    #[test]
    fn subagent_step_uses_own_tokens() {
        let mut steps = vec![make_step(
            "sub-1",
            SemanticStepType::Subagent,
            None,
            Some(StepTokens {
                input: 300,
                output: 200,
                cached: Some(100),
            }),
        )];

        calculate_step_context(&mut steps, &[]);

        assert_eq!(steps[0].accumulated_context, Some(400)); // 300 + 100
    }

    #[test]
    fn subagent_step_without_cached() {
        let mut steps = vec![make_step(
            "sub-2",
            SemanticStepType::Subagent,
            None,
            Some(StepTokens {
                input: 500,
                output: 100,
                cached: None,
            }),
        )];

        calculate_step_context(&mut steps, &[]);

        assert_eq!(steps[0].accumulated_context, Some(500)); // 500 + 0
    }

    #[test]
    fn subagent_step_without_tokens_stays_none() {
        let mut steps = vec![make_step(
            "sub-3",
            SemanticStepType::Subagent,
            None,
            None,
        )];

        calculate_step_context(&mut steps, &[]);

        assert_eq!(steps[0].accumulated_context, None);
    }

    #[test]
    fn other_step_accumulates_from_source_message() {
        let messages = vec![make_message(
            "msg-1",
            Some(UsageMetadata {
                input_tokens: 1000,
                output_tokens: 200,
                cache_read_input_tokens: Some(300),
                cache_creation_input_tokens: Some(50),
            }),
        )];

        let mut steps = vec![make_step(
            "s1",
            SemanticStepType::Thinking,
            Some("msg-1"),
            None,
        )];

        calculate_step_context(&mut steps, &messages);

        assert_eq!(steps[0].accumulated_context, Some(1350)); // 1000 + 300 + 50
    }

    #[test]
    fn other_step_without_cache_fields() {
        let messages = vec![make_message(
            "msg-2",
            Some(UsageMetadata {
                input_tokens: 800,
                output_tokens: 100,
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            }),
        )];

        let mut steps = vec![make_step(
            "s2",
            SemanticStepType::ToolCall,
            Some("msg-2"),
            None,
        )];

        calculate_step_context(&mut steps, &messages);

        assert_eq!(steps[0].accumulated_context, Some(800)); // 800 + 0 + 0
    }

    #[test]
    fn step_with_missing_source_message_skipped() {
        let messages = vec![make_message("msg-other", None)];

        let mut steps = vec![make_step(
            "s3",
            SemanticStepType::Output,
            Some("msg-nonexistent"),
            None,
        )];

        calculate_step_context(&mut steps, &messages);

        assert_eq!(steps[0].accumulated_context, None);
    }

    #[test]
    fn step_without_source_message_id_skipped() {
        let mut steps = vec![make_step(
            "s4",
            SemanticStepType::Thinking,
            None,
            None,
        )];

        calculate_step_context(&mut steps, &[]);

        assert_eq!(steps[0].accumulated_context, None);
    }

    #[test]
    fn step_with_empty_source_message_id_skipped() {
        let mut steps = vec![make_step(
            "s5",
            SemanticStepType::Output,
            Some(""),
            None,
        )];

        calculate_step_context(&mut steps, &[]);

        assert_eq!(steps[0].accumulated_context, None);
    }

    #[test]
    fn step_with_source_message_lacking_usage_skipped() {
        let messages = vec![make_message("msg-no-usage", None)];

        let mut steps = vec![make_step(
            "s6",
            SemanticStepType::ToolResult,
            Some("msg-no-usage"),
            None,
        )];

        calculate_step_context(&mut steps, &messages);

        assert_eq!(steps[0].accumulated_context, None);
    }

    #[test]
    fn mixed_steps() {
        let messages = vec![make_message(
            "msg-a",
            Some(UsageMetadata {
                input_tokens: 500,
                output_tokens: 50,
                cache_read_input_tokens: Some(100),
                cache_creation_input_tokens: None,
            }),
        )];

        let mut steps = vec![
            make_step(
                "sub-x",
                SemanticStepType::Subagent,
                None,
                Some(StepTokens {
                    input: 200,
                    output: 80,
                    cached: Some(50),
                }),
            ),
            make_step(
                "s-y",
                SemanticStepType::Thinking,
                Some("msg-a"),
                None,
            ),
        ];

        calculate_step_context(&mut steps, &messages);

        assert_eq!(steps[0].accumulated_context, Some(250)); // subagent: 200 + 50
        assert_eq!(steps[1].accumulated_context, Some(600)); // other: 500 + 100 + 0
    }
}
