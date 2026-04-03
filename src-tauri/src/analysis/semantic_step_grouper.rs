//! Semantic step grouping by source message.
//!
//! Groups semantic steps that originate from the same source message into
//! `SemanticStepGroup` instances, each with a human-readable label derived
//! from the types of steps in the group.

use std::collections::BTreeMap;

use crate::types::messages::{
    SemanticStep, SemanticStepGroup, SemanticStepType,
};

/// Build semantic step groups from a flat list of steps.
///
/// Steps are grouped by `source_message_id`.  Steps without a source
/// (empty or `None`) are each placed in their own group.
///
/// Group labels follow this priority:
/// 1. Contains tool calls       -> `"Tools ({count})"`
/// 2. Has thinking + output     -> `"Assistant Response"`
/// 3. Has only thinking         -> `"Thinking"`
/// 4. Has only output           -> `"Output"`
/// 5. Fallback                  -> `"Response ({count} steps)"`
pub fn build_semantic_step_groups(steps: &[SemanticStep]) -> Vec<SemanticStepGroup> {
    // BTreeMap preserves insertion order of keys for deterministic output.
    let mut groups: BTreeMap<String, Vec<&SemanticStep>> = BTreeMap::new();

    for step in steps {
        let key = step
            .source_message_id
            .as_deref()
            .unwrap_or("")
            .to_string();

        groups.entry(key).or_default().push(step);
    }

    groups
        .into_iter()
        .map(|(key, group_steps)| {
            let label = compute_group_label(&group_steps);
            let step_ids: Vec<String> = group_steps.iter().map(|s| s.id.clone()).collect();
            let id = if key.is_empty() {
                step_ids.first().cloned().unwrap_or_default()
            } else {
                key
            };

            SemanticStepGroup { id, label, step_ids }
        })
        .collect()
}

/// Determine the label for a group of steps based on the step types present.
fn compute_group_label(steps: &[&SemanticStep]) -> String {
    let has_tool_calls = steps
        .iter()
        .any(|s| s.step_type == SemanticStepType::ToolCall);

    let has_thinking = steps
        .iter()
        .any(|s| s.step_type == SemanticStepType::Thinking);

    let has_output = steps
        .iter()
        .any(|s| s.step_type == SemanticStepType::Output);

    let tool_call_count = steps
        .iter()
        .filter(|s| s.step_type == SemanticStepType::ToolCall)
        .count();

    if has_tool_calls {
        return format!("Tools ({})", tool_call_count);
    }

    if has_thinking && has_output {
        return "Assistant Response".to_string();
    }

    if has_thinking {
        return "Thinking".to_string();
    }

    if has_output {
        return "Output".to_string();
    }

    format!("Response ({} steps)", steps.len())
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
        step_type: SemanticStepType,
        source_message_id: Option<&str>,
    ) -> SemanticStep {
        SemanticStep {
            id: id.to_string(),
            step_type,
            start_time: 0,
            end_time: None,
            duration_ms: 0,
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
            source_message_id: source_message_id.map(|s| s.to_string()),
            effective_end_time: None,
            effective_duration_ms: None,
            is_gap_filled: None,
            context_tokens: None,
            accumulated_context: None,
            token_breakdown: None,
        }
    }

    #[test]
    fn empty_steps_produces_empty_groups() {
        let groups = build_semantic_step_groups(&[]);
        assert!(groups.is_empty());
    }

    #[test]
    fn single_step_creates_one_group() {
        let steps = vec![make_step("s1", SemanticStepType::Thinking, Some("msg-1"))];
        let groups = build_semantic_step_groups(&steps);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, "msg-1");
        assert_eq!(groups[0].label, "Thinking");
        assert_eq!(groups[0].step_ids, vec!["s1"]);
    }

    #[test]
    fn steps_with_same_source_grouped_together() {
        let steps = vec![
            make_step("s1", SemanticStepType::Thinking, Some("msg-a")),
            make_step("s2", SemanticStepType::ToolCall, Some("msg-a")),
            make_step("s3", SemanticStepType::Output, Some("msg-a")),
        ];
        let groups = build_semantic_step_groups(&steps);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].step_ids.len(), 3);
        assert_eq!(groups[0].label, "Tools (1)");
    }

    #[test]
    fn steps_with_different_sources_create_separate_groups() {
        let steps = vec![
            make_step("s1", SemanticStepType::Thinking, Some("msg-x")),
            make_step("s2", SemanticStepType::Output, Some("msg-y")),
        ];
        let groups = build_semantic_step_groups(&steps);

        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn tool_calls_label_with_count() {
        let steps = vec![
            make_step("t1", SemanticStepType::ToolCall, Some("msg-1")),
            make_step("t2", SemanticStepType::ToolCall, Some("msg-1")),
            make_step("t3", SemanticStepType::ToolCall, Some("msg-1")),
        ];
        let groups = build_semantic_step_groups(&steps);

        assert_eq!(groups[0].label, "Tools (3)");
    }

    #[test]
    fn thinking_and_output_label() {
        let steps = vec![
            make_step("s1", SemanticStepType::Thinking, Some("msg-1")),
            make_step("s2", SemanticStepType::Output, Some("msg-1")),
        ];
        let groups = build_semantic_step_groups(&steps);

        assert_eq!(groups[0].label, "Assistant Response");
    }

    #[test]
    fn thinking_only_label() {
        let steps = vec![make_step("s1", SemanticStepType::Thinking, Some("msg-1"))];
        let groups = build_semantic_step_groups(&steps);

        assert_eq!(groups[0].label, "Thinking");
    }

    #[test]
    fn output_only_label() {
        let steps = vec![make_step("s1", SemanticStepType::Output, Some("msg-1"))];
        let groups = build_semantic_step_groups(&steps);

        assert_eq!(groups[0].label, "Output");
    }

    #[test]
    fn fallback_response_label() {
        let steps = vec![
            make_step("s1", SemanticStepType::Interruption, Some("msg-1")),
            make_step("s2", SemanticStepType::ToolResult, Some("msg-1")),
        ];
        let groups = build_semantic_step_groups(&steps);

        assert_eq!(groups[0].label, "Response (2 steps)");
    }

    #[test]
    fn steps_without_source_message_id_get_own_groups() {
        let steps = vec![
            make_step("s1", SemanticStepType::Thinking, None),
            make_step("s2", SemanticStepType::Output, Some("msg-1")),
        ];
        let groups = build_semantic_step_groups(&steps);

        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn steps_with_empty_source_message_id_get_own_groups() {
        let steps = vec![
            make_step("s1", SemanticStepType::Thinking, Some("")),
            make_step("s2", SemanticStepType::Thinking, Some("")),
            make_step("s3", SemanticStepType::Output, Some("msg-1")),
        ];
        let groups = build_semantic_step_groups(&steps);

        assert_eq!(groups.len(), 2);
        // Empty source group uses first step id
        assert_eq!(groups[0].id, "s1");
        assert_eq!(groups[0].step_ids.len(), 2);
    }

    #[test]
    fn group_preserves_step_order() {
        let steps = vec![
            make_step("s3", SemanticStepType::Output, Some("msg-1")),
            make_step("s1", SemanticStepType::Thinking, Some("msg-1")),
            make_step("s2", SemanticStepType::ToolCall, Some("msg-1")),
        ];
        let groups = build_semantic_step_groups(&steps);

        assert_eq!(groups[0].step_ids, vec!["s3", "s1", "s2"]);
    }
}
