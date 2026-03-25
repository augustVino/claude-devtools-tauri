use crate::types::chunks::{AiChunk, Process};
use crate::types::domain::MessageType;
use crate::types::messages::{
    SemanticStep, SemanticStepContent, SemanticStepType, StepTokens,
};

/// Extract semantic steps from an AI chunk's responses and processes.
///
/// Semantic steps represent logical units of work within AI responses:
/// - thinking: Claude's reasoning process
/// - tool_call: Tool invocation
/// - output: Text output from Claude
/// - subagent: Nested agent execution
/// - interruption: User interruption / rejected tool use
///
/// Aligned with Electron's `SemanticStepExtractor.extractSemanticStepsFromAIChunk`.
pub fn extract_semantic_steps(chunk: &AiChunk) -> Vec<SemanticStep> {
    let mut steps: Vec<SemanticStep> = Vec::new();
    let mut step_id_counter = 0u32;

    for msg in &chunk.responses {
        // Extract from assistant responses (content blocks)
        if msg.message_type == MessageType::Assistant {
            let blocks = extract_content_blocks(&msg.content);

            for block in &blocks {
                match block {
                    ContentBlockRef::Thinking { thinking } => {
                        let token_count = count_tokens(thinking);
                        steps.push(SemanticStep {
                            id: format!("{}-thinking-{}", msg.uuid, step_id_counter),
                            step_type: SemanticStepType::Thinking,
                            start_time: msg.timestamp.clone(),
                            end_time: None,
                            duration_ms: 0,
                            content: SemanticStepContent {
                                thinking: Some(thinking.clone()),
                                ..Default::default()
                            },
                            tokens: Some(StepTokens {
                                input: 0,
                                output: token_count,
                                cached: None,
                            }),
                            is_parallel: None,
                            group_id: None,
                            context: if msg.agent_id.is_some() {
                                "subagent".to_string()
                            } else {
                                "main".to_string()
                            },
                            agent_id: msg.agent_id.clone(),
                            source_message_id: Some(msg.uuid.clone()),
                            effective_end_time: None,
                            effective_duration_ms: None,
                            is_gap_filled: None,
                            context_tokens: None,
                            accumulated_context: None,
                            token_breakdown: None,
                        });
                        step_id_counter += 1;
                    }
                    ContentBlockRef::ToolUse {
                        id,
                        name,
                        input,
                    } => {
                        let call_tokens =
                            count_tokens(&format!("{}{}", name, input));
                        steps.push(SemanticStep {
                            id: id.clone(),
                            step_type: SemanticStepType::ToolCall,
                            start_time: msg.timestamp.clone(),
                            end_time: None,
                            duration_ms: 0,
                            content: SemanticStepContent {
                                tool_use_id: Some(id.clone()),
                                tool_name: Some(name.clone()),
                                tool_input: Some(input.clone()),
                                ..Default::default()
                            },
                            tokens: Some(StepTokens {
                                input: call_tokens,
                                output: 0,
                                cached: None,
                            }),
                            is_parallel: None,
                            group_id: None,
                            context: if msg.agent_id.is_some() {
                                "subagent".to_string()
                            } else {
                                "main".to_string()
                            },
                            agent_id: msg.agent_id.clone(),
                            source_message_id: Some(msg.uuid.clone()),
                            effective_end_time: None,
                            effective_duration_ms: None,
                            is_gap_filled: None,
                            context_tokens: None,
                            accumulated_context: None,
                            token_breakdown: None,
                        });
                    }
                    ContentBlockRef::Text { text } => {
                        let text_tokens = count_tokens(text);
                        steps.push(SemanticStep {
                            id: format!("{}-output-{}", msg.uuid, step_id_counter),
                            step_type: SemanticStepType::Output,
                            start_time: msg.timestamp.clone(),
                            end_time: None,
                            duration_ms: 0,
                            content: SemanticStepContent {
                                text: Some(text.clone()),
                                ..Default::default()
                            },
                            tokens: Some(StepTokens {
                                input: 0,
                                output: text_tokens,
                                cached: None,
                            }),
                            is_parallel: None,
                            group_id: None,
                            context: if msg.agent_id.is_some() {
                                "subagent".to_string()
                            } else {
                                "main".to_string()
                            },
                            agent_id: msg.agent_id.clone(),
                            source_message_id: Some(msg.uuid.clone()),
                            effective_end_time: None,
                            effective_duration_ms: None,
                            is_gap_filled: None,
                            context_tokens: None,
                            accumulated_context: None,
                            token_breakdown: None,
                        });
                        step_id_counter += 1;
                    }
                    _ => {}
                }
            }
        }

        // Tool results from internal user messages
        if msg.message_type == MessageType::User && !msg.tool_results.is_empty() {
            for result in &msg.tool_results {
                let result_text = match &result.content {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                steps.push(SemanticStep {
                    id: result.tool_use_id.clone(),
                    step_type: SemanticStepType::ToolResult,
                    start_time: msg.timestamp.clone(),
                    end_time: None,
                    duration_ms: 0,
                    content: SemanticStepContent {
                        tool_output: Some(result.content.clone()),
                        tool_result_id: Some(result.tool_use_id.clone()),
                        tool_use_result: msg.tool_use_result.clone(),
                        text: Some(result_text),
                        ..Default::default()
                    },
                    tokens: None,
                    is_parallel: None,
                    group_id: None,
                    context: if msg.agent_id.is_some() {
                        "subagent".to_string()
                    } else {
                        "main".to_string()
                    },
                    agent_id: msg.agent_id.clone(),
                    source_message_id: Some(msg.uuid.clone()),
                    effective_end_time: None,
                    effective_duration_ms: None,
                    is_gap_filled: None,
                    context_tokens: None,
                    accumulated_context: None,
                    token_breakdown: None,
                });
            }
        }

        // Interruption: user messages with interruption text or rejected tool use
        if msg.message_type == MessageType::User {
            if let serde_json::Value::Array(arr) = &msg.content {
                let mut found_interruption = false;

                for block in arr {
                    let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if block_type != "text" {
                        continue;
                    }
                    let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");

                    if text.contains("[Request interrupted by user]")
                        || text.contains("[Request interrupted by user for tool use]")
                    {
                        steps.push(SemanticStep {
                            id: format!("{}-interruption-{}", msg.uuid, step_id_counter),
                            step_type: SemanticStepType::Interruption,
                            start_time: msg.timestamp.clone(),
                            end_time: None,
                            duration_ms: 0,
                            content: SemanticStepContent {
                                text: Some(text.to_string()),
                                ..Default::default()
                            },
                            tokens: None,
                            is_parallel: None,
                            group_id: None,
                            context: if msg.agent_id.is_some() {
                                "subagent".to_string()
                            } else {
                                "main".to_string()
                            },
                            agent_id: msg.agent_id.clone(),
                            source_message_id: Some(msg.uuid.clone()),
                            effective_end_time: None,
                            effective_duration_ms: None,
                            is_gap_filled: None,
                            context_tokens: None,
                            accumulated_context: None,
                            token_breakdown: None,
                        });
                        step_id_counter += 1;
                        found_interruption = true;
                    }
                }

                // User rejected tool use (no interruption text found)
                if !found_interruption {
                    if let Some(tool_use_result) = &msg.tool_use_result {
                        if tool_use_result.as_str() == Some("User rejected tool use") {
                            steps.push(SemanticStep {
                                id: format!("{}-interruption-{}", msg.uuid, step_id_counter),
                                step_type: SemanticStepType::Interruption,
                                start_time: msg.timestamp.clone(),
                                end_time: None,
                                duration_ms: 0,
                                content: SemanticStepContent {
                                    text: Some("Request interrupted by user".to_string()),
                                    ..Default::default()
                                },
                                tokens: None,
                                is_parallel: None,
                                group_id: None,
                                context: if msg.agent_id.is_some() {
                                    "subagent".to_string()
                                } else {
                                    "main".to_string()
                                },
                                agent_id: msg.agent_id.clone(),
                                source_message_id: Some(msg.uuid.clone()),
                                effective_end_time: None,
                                effective_duration_ms: None,
                                is_gap_filled: None,
                                context_tokens: None,
                                accumulated_context: None,
                                token_breakdown: None,
                            });
                            step_id_counter += 1;
                        }
                    }
                }
            }
        }
    }

    // Subagent steps from processes
    for process in &chunk.processes {
        steps.push(build_subagent_step(process));
    }

    // Sort by start_time
    steps.sort_by(|a, b| a.start_time.cmp(&b.start_time));

    steps
}

/// Build a subagent semantic step from a Process.
fn build_subagent_step(process: &Process) -> SemanticStep {
    SemanticStep {
        id: process.id.clone(),
        step_type: SemanticStepType::Subagent,
        start_time: format_timestamp_ms(process.start_time),
        end_time: Some(format_timestamp_ms(process.end_time)),
        duration_ms: process.duration_ms,
        content: SemanticStepContent {
            subagent_id: Some(process.id.clone()),
            subagent_description: process.description.clone(),
            ..Default::default()
        },
        tokens: Some(StepTokens {
            input: process.metrics.input_tokens,
            output: process.metrics.output_tokens,
            cached: process.metrics.cache_read_tokens,
        }),
        is_parallel: Some(process.is_parallel),
        group_id: None,
        context: "subagent".to_string(),
        agent_id: Some(process.id.clone()),
        source_message_id: None,
        effective_end_time: None,
        effective_duration_ms: None,
        is_gap_filled: None,
        context_tokens: None,
        accumulated_context: None,
        token_breakdown: None,
    }
}

/// Estimate token count from character length.
/// Uses the standard heuristic: `char_length / 4`.
fn count_tokens(text: &str) -> u64 {
    ((text.len() as f64) / 4.0).ceil() as u64
}

/// Format a millisecond timestamp to ISO 8601 string.
fn format_timestamp_ms(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

// =============================================================================
// Content Block Extraction
// =============================================================================

/// Lightweight content block representation extracted from JSON.
enum ContentBlockRef {
    Thinking {
        thinking: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    Text {
        text: String,
    },
    ToolResult {
        tool_use_id: String,
    },
    Image,
}

/// Extract typed content blocks from a ParsedMessage's content field.
/// The content field is a `serde_json::Value` that can be either a string or array.
fn extract_content_blocks(content: &serde_json::Value) -> Vec<ContentBlockRef> {
    let arr = match content {
        serde_json::Value::Array(arr) => arr,
        serde_json::Value::String(s) if s.is_empty() => return Vec::new(),
        serde_json::Value::String(_) => return Vec::new(),
        _ => return Vec::new(),
    };

    arr.iter()
        .filter_map(|block| {
            let block_type = block.get("type").and_then(|t| t.as_str())?;

            match block_type {
                "thinking" => {
                    let thinking = block.get("thinking")?.as_str()?;
                    if thinking.is_empty() {
                        return None;
                    }
                    Some(ContentBlockRef::Thinking { thinking: thinking.to_string() })
                }
                "tool_use" => {
                    let id = block.get("id")?.as_str()?.to_string();
                    let name = block.get("name")?.as_str()?.to_string();
                    let input = block.get("input").cloned().unwrap_or(serde_json::Value::Null);
                    Some(ContentBlockRef::ToolUse { id, name, input })
                }
                "text" => {
                    let text = block.get("text")?.as_str()?;
                    if text.is_empty() {
                        return None;
                    }
                    Some(ContentBlockRef::Text { text: text.to_string() })
                }
                "tool_result" => {
                    let tool_use_id = block
                        .get("tool_use_id")?
                        .as_str()?
                        .to_string();
                    Some(ContentBlockRef::ToolResult { tool_use_id })
                }
                "image" => Some(ContentBlockRef::Image),
                _ => None,
            }
        })
        .collect()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::chunks::Process;
    use crate::types::domain::{MessageType, SessionMetrics};
    use crate::types::messages::{
        ParsedMessage, SemanticStepType, ToolResult,
    };

    // =========================================================================
    // Test Helpers
    // =========================================================================

    fn make_assistant_msg(
        uuid: &str,
        timestamp: &str,
        content: serde_json::Value,
    ) -> ParsedMessage {
        ParsedMessage {
            uuid: uuid.to_string(),
            parent_uuid: None,
            message_type: MessageType::Assistant,
            timestamp: timestamp.to_string(),
            role: Some("assistant".to_string()),
            content,
            usage: None,
            model: Some("claude-sonnet-4-20250514".to_string()),
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

    fn make_user_msg(
        uuid: &str,
        timestamp: &str,
        content: serde_json::Value,
        is_meta: bool,
    ) -> ParsedMessage {
        ParsedMessage {
            uuid: uuid.to_string(),
            parent_uuid: None,
            message_type: MessageType::User,
            timestamp: timestamp.to_string(),
            role: Some("user".to_string()),
            content,
            usage: None,
            model: None,
            cwd: None,
            git_branch: None,
            agent_id: None,
            is_sidechain: false,
            is_meta,
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

    fn empty_chunk() -> AiChunk {
        AiChunk {
            id: "chunk-1".to_string(),
            start_time: 0,
            end_time: 0,
            duration_ms: 0,
            metrics: SessionMetrics::default(),
            responses: vec![],
            processes: vec![],
            sidechain_messages: vec![],
            tool_executions: vec![],
            semantic_steps: vec![],
            semantic_step_groups: vec![],
        }
    }

    // =========================================================================
    // Thinking Steps
    // =========================================================================

    #[test]
    fn extract_thinking_step_from_assistant_response() {
        let response = make_assistant_msg(
            "resp-1",
            "2026-03-25T10:00:00.000Z",
            serde_json::json!([
                {
                    "type": "thinking",
                    "thinking": "Let me analyze this problem carefully",
                    "signature": "sig-123"
                },
                {
                    "type": "text",
                    "text": "Here is my analysis"
                }
            ]),
        );

        let chunk = AiChunk {
            responses: vec![response],
            ..empty_chunk()
        };

        let steps = extract_semantic_steps(&chunk);

        let thinking_step = steps
            .iter()
            .find(|s| s.step_type == SemanticStepType::Thinking);
        assert!(thinking_step.is_some(), "Should have a thinking step");

        let step = thinking_step.unwrap();
        assert_eq!(
            step.content.thinking.as_deref(),
            Some("Let me analyze this problem carefully")
        );
        assert_eq!(
            step.source_message_id.as_deref(),
            Some("resp-1")
        );
        assert_eq!(step.context, "main");
        assert_eq!(step.tokens.as_ref().unwrap().output, 10); // "Let me analyze this problem carefully" = 42 chars -> ceil(42/4) = 11
        assert!(step.tokens.as_ref().unwrap().cached.is_none());
    }

    #[test]
    fn extract_thinking_step_with_subagent_context() {
        let mut response = make_assistant_msg(
            "resp-sub-1",
            "2026-03-25T10:00:00.000Z",
            serde_json::json!([
                {
                    "type": "thinking",
                    "thinking": "Working on subtask",
                    "signature": "sig-456"
                }
            ]),
        );
        response.agent_id = Some("agent-123".to_string());

        let chunk = AiChunk {
            responses: vec![response],
            ..empty_chunk()
        };

        let steps = extract_semantic_steps(&chunk);
        let thinking_step = steps
            .iter()
            .find(|s| s.step_type == SemanticStepType::Thinking)
            .unwrap();

        assert_eq!(thinking_step.context, "subagent");
        assert_eq!(
            thinking_step.agent_id.as_deref(),
            Some("agent-123")
        );
    }

    // =========================================================================
    // Tool Call Steps
    // =========================================================================

    #[test]
    fn extract_tool_call_step_from_assistant_response() {
        let response = make_assistant_msg(
            "resp-2",
            "2026-03-25T10:00:01.000Z",
            serde_json::json!([
                {
                    "type": "tool_use",
                    "id": "tu-abc-123",
                    "name": "Read",
                    "input": {"file_path": "/tmp/test.txt"}
                }
            ]),
        );

        let chunk = AiChunk {
            responses: vec![response],
            ..empty_chunk()
        };

        let steps = extract_semantic_steps(&chunk);

        let tool_call_step = steps
            .iter()
            .find(|s| s.step_type == SemanticStepType::ToolCall);
        assert!(tool_call_step.is_some(), "Should have a tool_call step");

        let step = tool_call_step.unwrap();
        assert_eq!(step.id, "tu-abc-123");
        assert_eq!(
            step.content.tool_name.as_deref(),
            Some("Read")
        );
        assert_eq!(
            step.content.tool_use_id.as_deref(),
            Some("tu-abc-123")
        );
        assert_eq!(
            step.content.tool_input.as_ref().unwrap().get("file_path"),
            Some(&serde_json::json!("/tmp/test.txt"))
        );
        assert_eq!(step.context, "main");
        assert_eq!(
            step.source_message_id.as_deref(),
            Some("resp-2")
        );
        assert!(step.tokens.as_ref().unwrap().input > 0);
        assert_eq!(step.tokens.as_ref().unwrap().output, 0);
    }

    // =========================================================================
    // Output Steps
    // =========================================================================

    #[test]
    fn extract_output_step_from_assistant_response() {
        let response = make_assistant_msg(
            "resp-3",
            "2026-03-25T10:00:02.000Z",
            serde_json::json!([
                {
                    "type": "text",
                    "text": "Here is the analysis result."
                }
            ]),
        );

        let chunk = AiChunk {
            responses: vec![response],
            ..empty_chunk()
        };

        let steps = extract_semantic_steps(&chunk);

        let output_step = steps
            .iter()
            .find(|s| s.step_type == SemanticStepType::Output);
        assert!(output_step.is_some(), "Should have an output step");

        let step = output_step.unwrap();
        assert_eq!(
            step.content.text.as_deref(),
            Some("Here is the analysis result.")
        );
        assert_eq!(
            step.source_message_id.as_deref(),
            Some("resp-3")
        );
        assert_eq!(step.context, "main");
        assert!(step.tokens.as_ref().unwrap().output > 0);
        assert_eq!(step.tokens.as_ref().unwrap().input, 0);
    }

    // =========================================================================
    // Multiple Steps + Sorting
    // =========================================================================

    #[test]
    fn extract_multiple_steps_sorted_by_start_time() {
        let response1 = make_assistant_msg(
            "resp-early",
            "2026-03-25T10:00:00.000Z",
            serde_json::json!([
                {
                    "type": "thinking",
                    "thinking": "Initial thought",
                    "signature": "sig-1"
                }
            ]),
        );

        let response2 = make_assistant_msg(
            "resp-late",
            "2026-03-25T10:00:05.000Z",
            serde_json::json!([
                {
                    "type": "tool_use",
                    "id": "tu-xyz",
                    "name": "Bash",
                    "input": {"command": "ls"}
                },
                {
                    "type": "text",
                    "text": "Done listing files."
                }
            ]),
        );

        let chunk = AiChunk {
            responses: vec![response2, response1], // Intentionally reversed order
            ..empty_chunk()
        };

        let steps = extract_semantic_steps(&chunk);

        assert_eq!(steps.len(), 3, "Should have 3 steps total");

        // Verify sort order: thinking (10:00:00), tool_call (10:00:05), output (10:00:05)
        assert_eq!(steps[0].step_type, SemanticStepType::Thinking);
        assert_eq!(steps[1].step_type, SemanticStepType::ToolCall);
        assert_eq!(steps[2].step_type, SemanticStepType::Output);

        assert!(steps[0].start_time <= steps[1].start_time);
        assert!(steps[1].start_time <= steps[2].start_time);
    }

    // =========================================================================
    // Tool Result Steps
    // =========================================================================

    #[test]
    fn extract_tool_result_from_internal_user_message() {
        let mut user_msg = make_user_msg(
            "user-1",
            "2026-03-25T10:00:03.000Z",
            serde_json::json!([
                {
                    "type": "tool_result",
                    "tool_use_id": "tu-abc-123",
                    "content": "file contents here",
                    "is_error": false
                }
            ]),
            true,
        );
        user_msg.tool_results = vec![ToolResult {
            tool_use_id: "tu-abc-123".to_string(),
            content: serde_json::Value::String("file contents here".to_string()),
            is_error: false,
        }];

        let chunk = AiChunk {
            responses: vec![user_msg],
            ..empty_chunk()
        };

        let steps = extract_semantic_steps(&chunk);

        let tool_result_step = steps
            .iter()
            .find(|s| s.step_type == SemanticStepType::ToolResult);
        assert!(
            tool_result_step.is_some(),
            "Should have a tool_result step"
        );

        let step = tool_result_step.unwrap();
        assert_eq!(step.id, "tu-abc-123");
        assert_eq!(
            step.content.tool_result_id.as_deref(),
            Some("tu-abc-123")
        );
    }

    // =========================================================================
    // Interruption Steps
    // =========================================================================

    #[test]
    fn extract_interruption_from_user_message() {
        let user_msg = make_user_msg(
            "user-int-1",
            "2026-03-25T10:00:04.000Z",
            serde_json::json!([
                {
                    "type": "text",
                    "text": "[Request interrupted by user]"
                }
            ]),
            false,
        );

        let chunk = AiChunk {
            responses: vec![user_msg],
            ..empty_chunk()
        };

        let steps = extract_semantic_steps(&chunk);

        let interruption_step = steps
            .iter()
            .find(|s| s.step_type == SemanticStepType::Interruption);
        assert!(
            interruption_step.is_some(),
            "Should have an interruption step"
        );

        let step = interruption_step.unwrap();
        assert_eq!(
            step.content.text.as_deref(),
            Some("[Request interrupted by user]")
        );
    }

    #[test]
    fn extract_interruption_for_tool_use() {
        let user_msg = make_user_msg(
            "user-int-2",
            "2026-03-25T10:00:04.000Z",
            serde_json::json!([
                {
                    "type": "text",
                    "text": "[Request interrupted by user for tool use]"
                }
            ]),
            false,
        );

        let chunk = AiChunk {
            responses: vec![user_msg],
            ..empty_chunk()
        };

        let steps = extract_semantic_steps(&chunk);

        let interruption_step = steps
            .iter()
            .find(|s| s.step_type == SemanticStepType::Interruption);
        assert!(interruption_step.is_some());
    }

    #[test]
    fn extract_interruption_from_rejected_tool_use() {
        let mut user_msg = make_user_msg(
            "user-reject-1",
            "2026-03-25T10:00:04.000Z",
            serde_json::json!([
                {
                    "type": "text",
                    "text": "some other text"
                }
            ]),
            false,
        );
        user_msg.tool_use_result =
            Some(serde_json::Value::String("User rejected tool use".to_string()));

        let chunk = AiChunk {
            responses: vec![user_msg],
            ..empty_chunk()
        };

        let steps = extract_semantic_steps(&chunk);

        let interruption_step = steps
            .iter()
            .find(|s| s.step_type == SemanticStepType::Interruption);
        assert!(
            interruption_step.is_some(),
            "Should have an interruption step from rejected tool use"
        );

        let step = interruption_step.unwrap();
        assert_eq!(
            step.content.text.as_deref(),
            Some("Request interrupted by user")
        );
    }

    // =========================================================================
    // Subagent Steps
    // =========================================================================

    #[test]
    fn extract_subagent_step_from_processes() {
        let process = Process {
            id: "proc-1".to_string(),
            file_path: "/tmp/subagent.jsonl".to_string(),
            description: Some("Research task".to_string()),
            subagent_type: None,
            messages: vec![],
            start_time: 1700000000000,
            end_time: 1700000010000,
            duration_ms: 10000,
            metrics: SessionMetrics {
                duration_ms: 10000,
                total_tokens: 500,
                input_tokens: 300,
                output_tokens: 200,
                cache_read_tokens: Some(100),
                cache_creation_tokens: None,
                message_count: 5,
                cost_usd: None,
            },
            is_parallel: true,
            parent_task_id: None,
            is_ongoing: None,
            main_session_impact: None,
            team: None,
        };

        let chunk = AiChunk {
            processes: vec![process],
            ..empty_chunk()
        };

        let steps = extract_semantic_steps(&chunk);

        assert_eq!(steps.len(), 1, "Should have 1 subagent step");

        let step = &steps[0];
        assert_eq!(step.step_type, SemanticStepType::Subagent);
        assert_eq!(step.id, "proc-1");
        assert_eq!(
            step.content.subagent_id.as_deref(),
            Some("proc-1")
        );
        assert_eq!(
            step.content.subagent_description.as_deref(),
            Some("Research task")
        );
        assert_eq!(step.context, "subagent");
        assert_eq!(step.is_parallel, Some(true));
        assert_eq!(step.tokens.as_ref().unwrap().input, 300);
        assert_eq!(step.tokens.as_ref().unwrap().output, 200);
        assert_eq!(
            step.tokens.as_ref().unwrap().cached,
            Some(100)
        );
    }

    // =========================================================================
    // Token Estimation
    // =========================================================================

    #[test]
    fn count_tokens_uses_char_length_div_four() {
        // 8 chars -> ceil(8/4) = 2
        assert_eq!(count_tokens("abcdefgh"), 2);
        // 10 chars -> ceil(10/4) = 3
        assert_eq!(count_tokens("abcdefghij"), 3);
        // 1 char -> ceil(1/4) = 1
        assert_eq!(count_tokens("a"), 1);
        // empty -> 0
        assert_eq!(count_tokens(""), 0);
    }

    // =========================================================================
    // Edge Cases
    // =========================================================================

    #[test]
    fn empty_chunk_produces_no_steps() {
        let chunk = empty_chunk();
        let steps = extract_semantic_steps(&chunk);
        assert!(steps.is_empty());
    }

    #[test]
    fn string_content_in_assistant_message_produces_no_steps() {
        // String content (not array) should not produce steps
        let response = make_assistant_msg(
            "resp-str",
            "2026-03-25T10:00:00.000Z",
            serde_json::Value::String("plain text response".to_string()),
        );

        let chunk = AiChunk {
            responses: vec![response],
            ..empty_chunk()
        };

        let steps = extract_semantic_steps(&chunk);
        assert!(steps.is_empty());
    }

    #[test]
    fn empty_content_blocks_are_skipped() {
        let response = make_assistant_msg(
            "resp-empty",
            "2026-03-25T10:00:00.000Z",
            serde_json::json!([
                {"type": "thinking", "thinking": "", "signature": "sig"},
                {"type": "text", "text": ""}
            ]),
        );

        let chunk = AiChunk {
            responses: vec![response],
            ..empty_chunk()
        };

        let steps = extract_semantic_steps(&chunk);
        // Empty thinking/text blocks are skipped by filter_map (get("thinking")? returns None)
        assert!(steps.is_empty());
    }

    #[test]
    fn interruption_text_takes_priority_over_rejected_tool_use() {
        // If interruption text is found in content, rejected tool use should not create a second step
        let mut user_msg = make_user_msg(
            "user-both",
            "2026-03-25T10:00:04.000Z",
            serde_json::json!([
                {
                    "type": "text",
                    "text": "[Request interrupted by user]"
                }
            ]),
            false,
        );
        user_msg.tool_use_result =
            Some(serde_json::Value::String("User rejected tool use".to_string()));

        let chunk = AiChunk {
            responses: vec![user_msg],
            ..empty_chunk()
        };

        let steps = extract_semantic_steps(&chunk);
        let interruption_count = steps
            .iter()
            .filter(|s| s.step_type == SemanticStepType::Interruption)
            .count();
        assert_eq!(
            interruption_count, 1,
            "Should have exactly 1 interruption step (not duplicated)"
        );
    }
}
