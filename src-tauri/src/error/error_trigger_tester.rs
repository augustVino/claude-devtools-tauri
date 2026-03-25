//! Error trigger tester -- tests trigger configurations against historical
//! session data.
//!
//! Provides the `test_trigger` function that scans all projects/sessions and
//! checks each message against a trigger.  Stops early when enough errors are
//! found or safety limits are hit.
//!
//! Ported from Electron `src/main/services/error/ErrorTriggerTester.ts`.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use crate::analysis::tool_extraction::{build_tool_result_map, build_tool_use_map};
use crate::discovery::project_scanner::ProjectScanner;
use crate::error::error_trigger_checker::{
    check_token_threshold_trigger, check_tool_result_trigger, check_tool_use_trigger,
    matches_repository_scope, pre_resolve_repository_ids, RepositoryScopeTarget,
};
use crate::parsing::jsonl_parser::parse_jsonl_file;
use crate::types::config::{DetectedError, NotificationTrigger, TriggerTestError, TriggerTestResult};

// =============================================================================
// Safety Limits
// =============================================================================

/// Maximum number of errors to return (primary stop condition).
const MAX_ERRORS: usize = 50;

/// Maximum total count to track (prevents indefinite counting).
const MAX_TOTAL_COUNT: u32 = 10_000;

/// Maximum time before aborting (main safety limit).
const TIMEOUT_MS: u128 = 30_000;

// =============================================================================
// Internal State
// =============================================================================

/// Mutable state tracked during trigger testing.
struct TestState {
    errors: Vec<DetectedError>,
    total_count: u32,
    sessions_scanned: u32,
    truncated: bool,
    start_time: Instant,
    effective_limit: usize,
}

impl TestState {
    fn new(limit: Option<usize>) -> Self {
        Self {
            errors: Vec::new(),
            total_count: 0,
            sessions_scanned: 0,
            truncated: false,
            start_time: Instant::now(),
            effective_limit: limit.unwrap_or(MAX_ERRORS).min(MAX_ERRORS),
        }
    }

    /// Returns `true` if we have collected enough errors (success case).
    fn has_enough_errors(&self) -> bool {
        self.errors.len() >= self.effective_limit
    }

    /// Check safety limits.  Returns `Some(reason)` if the test should stop,
    /// `None` if it should continue.
    fn check_safety_limits(&self) -> Option<&'static str> {
        if self.start_time.elapsed().as_millis() > TIMEOUT_MS {
            return Some("Trigger test timed out after 30 seconds");
        }
        if self.total_count >= MAX_TOTAL_COUNT {
            return Some("Trigger test stopped after reaching count limit");
        }
        None
    }
}

// =============================================================================
// Public API
// =============================================================================

/// Test a trigger against all projects/sessions.
///
/// Scans sessions until enough results are found or safety limits are hit.
///
/// # Arguments
/// * `trigger` - The trigger configuration to test.
/// * `project_scanner` - Scanner for discovering projects and sessions.
/// * `limit` - Maximum number of results to return (capped at `MAX_ERRORS`).
///
/// # Returns
/// A [`TriggerTestResult`] containing matched errors and metadata.
pub async fn test_trigger(
    trigger: &NotificationTrigger,
    project_scanner: &ProjectScanner,
    limit: Option<usize>,
) -> TriggerTestResult {
    let mut state = TestState::new(limit);

    let result = run_test(trigger, project_scanner, &mut state).await;

    match result {
        Ok(()) => TriggerTestResult {
            total_count: state.total_count,
            truncated: if state.truncated { Some(true) } else { None },
            errors: state
                .errors
                .into_iter()
                .take(state.effective_limit)
                .map(detected_to_test_error)
                .collect(),
        },
        Err(_) => TriggerTestResult {
            total_count: 0,
            truncated: None,
            errors: Vec::new(),
        },
    }
}

// =============================================================================
// Core Logic
// =============================================================================

/// Main test loop: scan projects and sessions, checking each message against
/// the trigger.
async fn run_test(
    trigger: &NotificationTrigger,
    project_scanner: &ProjectScanner,
    state: &mut TestState,
) -> Result<(), ()> {
    let projects = project_scanner.scan();

    for project in &projects {
        // Check safety limits before processing each project.
        if let Some(reason) = state.check_safety_limits() {
            log::warn!("{}", reason);
            state.truncated = true;
            break;
        }

        // Early exit if we have enough errors (success, no truncation).
        if state.has_enough_errors() {
            break;
        }

        let session_files = project_scanner.list_session_files(&project.id);

        // Pre-resolve repository ID for this project.
        pre_resolve_repository_ids(&[RepositoryScopeTarget {
            project_id: project.id.clone(),
            cwd_hint: Some(project.path.clone()),
        }]);

        // Process each session file.
        let should_break = process_session_files(
            &session_files,
            trigger,
            &project.id,
            state,
        )
        .await;

        if should_break {
            break;
        }
    }

    Ok(())
}

/// Processes session files for a single project.
///
/// Returns `true` if the outer project loop should break.
async fn process_session_files(
    session_files: &[String],
    trigger: &NotificationTrigger,
    project_id: &str,
    state: &mut TestState,
) -> bool {
    for file_path in session_files {
        // Check safety limits.
        if let Some(reason) = state.check_safety_limits() {
            log::warn!("{}", reason);
            state.truncated = true;
            return true;
        }

        // Early exit if we have enough errors.
        if state.has_enough_errors() {
            return false;
        }

        state.sessions_scanned += 1;

        // Parse session file.
        let path = Path::new(file_path);
        let messages = parse_jsonl_file(path).await;

        // Extract session ID from file name.
        let session_id = Path::new(file_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        // Detect errors in this session.
        let session_errors =
            detect_errors_with_trigger(&messages, trigger, &session_id, project_id, file_path);

        // Update total count (capped).
        let new_total = state.total_count + session_errors.len() as u32;
        if new_total >= MAX_TOTAL_COUNT {
            state.total_count = MAX_TOTAL_COUNT;
            state.truncated = true;
        } else {
            state.total_count = new_total;
        }

        // Add errors up to the effective limit.
        for error in session_errors {
            if state.has_enough_errors() {
                break;
            }
            state.errors.push(error);
        }
    }

    false
}

/// Detects errors from messages using a single trigger.
fn detect_errors_with_trigger(
    messages: &[crate::types::messages::ParsedMessage],
    trigger: &NotificationTrigger,
    session_id: &str,
    project_id: &str,
    file_path: &str,
) -> Vec<DetectedError> {
    let mut errors = Vec::new();

    // Build tool_use map for linking results to calls.
    let tool_use_map = build_tool_use_map(messages);
    // Build tool_result map for estimating output tokens.
    let tool_result_map = build_tool_result_map(messages);

    for (i, message) in messages.iter().enumerate() {
        let line_number = (i + 1) as u64; // 1-based line numbers for JSONL

        let trigger_errors = check_trigger(
            message,
            trigger,
            &tool_use_map,
            &tool_result_map,
            session_id,
            project_id,
            file_path,
            line_number,
        );

        errors.extend(trigger_errors);
    }

    errors
}

/// Checks if a message matches a specific trigger.
///
/// Returns a vector of detected errors (can be multiple for
/// `token_threshold` mode).
fn check_trigger(
    message: &crate::types::messages::ParsedMessage,
    trigger: &NotificationTrigger,
    tool_use_map: &HashMap<String, crate::analysis::tool_extraction::ToolUseInfo>,
    tool_result_map: &HashMap<String, crate::analysis::tool_extraction::ToolResultInfo>,
    session_id: &str,
    project_id: &str,
    file_path: &str,
    line_number: u64,
) -> Vec<DetectedError> {
    // Check repository scope first.
    if !matches_repository_scope(project_id, trigger.repository_ids.as_deref()) {
        return Vec::new();
    }

    use crate::types::config::{TriggerContentType, TriggerMode};

    // Handle token_threshold mode.
    if trigger.mode == TriggerMode::TokenThreshold {
        return check_token_threshold_trigger(
            message,
            trigger,
            tool_result_map,
            session_id,
            project_id,
            file_path,
            line_number,
        );
    }

    // Handle tool_result triggers.
    if trigger.content_type == TriggerContentType::ToolResult {
        if let Some(error) = check_tool_result_trigger(
            message,
            trigger,
            tool_use_map,
            session_id,
            project_id,
            file_path,
            line_number,
        ) {
            return vec![error];
        }
        return Vec::new();
    }

    // Handle tool_use triggers.
    if trigger.content_type == TriggerContentType::ToolUse {
        if let Some(error) = check_tool_use_trigger(
            message,
            trigger,
            session_id,
            project_id,
            file_path,
            line_number,
        ) {
            return vec![error];
        }
        return Vec::new();
    }

    Vec::new()
}

// =============================================================================
// Conversion
// =============================================================================

/// Converts a [`DetectedError`] to a [`TriggerTestError`] (stripped version
/// for the test result).
fn detected_to_test_error(error: DetectedError) -> TriggerTestError {
    TriggerTestError {
        id: error.id,
        session_id: error.session_id,
        project_id: error.project_id,
        message: error.message,
        timestamp: error.timestamp,
        source: error.source,
        tool_use_id: error.tool_use_id,
        subagent_id: error.subagent_id,
        line_number: error.line_number,
        context: error.context,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::config::{TriggerContentType, TriggerMode};
    use std::fs;

    // ---------------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------------

    fn make_error_trigger() -> NotificationTrigger {
        NotificationTrigger {
            id: "test-trigger".to_string(),
            name: "Test Trigger".to_string(),
            enabled: true,
            content_type: TriggerContentType::ToolResult,
            tool_name: None,
            is_builtin: None,
            ignore_patterns: None,
            mode: TriggerMode::ErrorStatus,
            require_error: Some(true),
            match_field: None,
            match_pattern: None,
            token_threshold: None,
            token_type: None,
            repository_ids: None,
            color: None,
        }
    }

    fn setup_test_env() -> (tempfile::TempDir, ProjectScanner) {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let projects_dir = temp_dir.path().join("projects");
        let todos_dir = temp_dir.path().join("todos");
        fs::create_dir_all(&projects_dir).unwrap();
        fs::create_dir_all(&todos_dir).unwrap();

        let scanner = ProjectScanner::with_paths(projects_dir, todos_dir);
        (temp_dir, scanner)
    }

    fn create_session_with_error(
        projects_dir: &std::path::Path,
        project_id: &str,
        session_id: &str,
        error_text: &str,
    ) {
        let project_dir = projects_dir.join(project_id);
        fs::create_dir_all(&project_dir).unwrap();

        let line1 = r#"{"type":"assistant","uuid":"a1","timestamp":"2026-03-25T06:00:01.000Z","message":{"role":"assistant","id":"msg_1","type":"message","model":"claude-3-opus","content":[{"type":"tool_use","id":"tu_1","name":"Bash","input":{"command":"npm build"}}],"stop_reason":"tool_use","usage":{"input_tokens":100,"output_tokens":50}},"isSidechain":false,"userType":"external"}"#;
        let line2 = format!(
            r#"{{"type":"user","uuid":"u1","timestamp":"2026-03-25T06:00:02.000Z","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"tu_1","content":"{}","is_error":true}}]}},"isMeta":true,"isSidechain":false,"sourceToolUseId":"tu_1","toolUseResult":{{"toolName":"Bash","output":"ok"}}}}"#,
            error_text
        );

        let session_path = project_dir.join(format!("{}.jsonl", session_id));
        fs::write(&session_path, format!("{}\n{}\n", line1, line2)).unwrap();
    }

    // ---------------------------------------------------------------------------
    // TestState tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_state_new_with_limit() {
        let state = TestState::new(Some(10));
        assert_eq!(state.effective_limit, 10);
    }

    #[test]
    fn test_state_new_with_excessive_limit() {
        let state = TestState::new(Some(1000));
        assert_eq!(state.effective_limit, MAX_ERRORS);
    }

    #[test]
    fn test_state_new_without_limit() {
        let state = TestState::new(None);
        assert_eq!(state.effective_limit, MAX_ERRORS);
    }

    #[test]
    fn test_state_has_enough_errors() {
        let mut state = TestState::new(Some(2));
        assert!(!state.has_enough_errors());

        state.errors.push(create_test_detected_error("e1"));
        assert!(!state.has_enough_errors());

        state.errors.push(create_test_detected_error("e2"));
        assert!(state.has_enough_errors());
    }

    // ---------------------------------------------------------------------------
    // detected_to_test_error tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_detected_to_test_error_conversion() {
        let detected = DetectedError {
            id: "err-1".to_string(),
            session_id: "sess-1".to_string(),
            project_id: "proj-1".to_string(),
            file_path: "/path/to/session.jsonl".to_string(),
            source: "Bash".to_string(),
            message: "Build failed".to_string(),
            timestamp: 1700000000000,
            line_number: Some(42),
            tool_use_id: Some("tu_1".to_string()),
            subagent_id: Some("agent-1".to_string()),
            trigger_color: Some("red".to_string()),
            trigger_id: Some("trigger-1".to_string()),
            trigger_name: Some("Build Errors".to_string()),
            context: crate::types::config::ErrorContext {
                project_name: "my-project".to_string(),
                cwd: Some("/home/user".to_string()),
            },
        };

        let test_error = detected_to_test_error(detected);

        assert_eq!(test_error.id, "err-1");
        assert_eq!(test_error.session_id, "sess-1");
        assert_eq!(test_error.project_id, "proj-1");
        assert_eq!(test_error.message, "Build failed");
        assert_eq!(test_error.timestamp, 1700000000000);
        assert_eq!(test_error.source, "Bash");
        assert_eq!(test_error.tool_use_id, Some("tu_1".to_string()));
        assert_eq!(test_error.subagent_id, Some("agent-1".to_string()));
        assert_eq!(test_error.line_number, Some(42));
        assert_eq!(test_error.context.project_name, "my-project");
    }

    // ---------------------------------------------------------------------------
    // Integration tests
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_test_trigger_finds_errors() {
        let (temp_dir, scanner) = setup_test_env();

        // Write the session file manually with compact JSON (no indentation).
        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();

        let line1 = r#"{"type":"assistant","uuid":"a1","timestamp":"2026-03-25T06:00:01.000Z","message":{"role":"assistant","id":"msg_1","type":"message","model":"claude-3-opus","content":[{"type":"tool_use","id":"tu_1","name":"Bash","input":{"command":"npm build"}}],"stop_reason":"tool_use","usage":{"input_tokens":100,"output_tokens":50}},"isSidechain":false,"userType":"external"}"#;
        let line2 = r#"{"type":"user","uuid":"u1","timestamp":"2026-03-25T06:00:02.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","content":"Build failed: error E0425","is_error":true}]},"isMeta":true,"isSidechain":false,"sourceToolUseId":"tu_1","toolUseResult":{"toolName":"Bash","output":"ok"}}"#;

        fs::write(
            project_dir.join("session-1.jsonl"),
            format!("{}\n{}", line1, line2),
        )
        .unwrap();

        let trigger = make_error_trigger();
        let result = test_trigger(&trigger, &scanner, None).await;

        assert_eq!(result.total_count, 1);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].source, "Bash");
        assert!(result.errors[0].message.contains("E0425"));
        assert!(result.truncated.is_none());
    }

    #[tokio::test]
    async fn test_test_trigger_no_matching_errors() {
        let (temp_dir, scanner) = setup_test_env();

        // Create a session without errors (is_error: false)
        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();

        let line1 = r#"{"type":"assistant","uuid":"a1","timestamp":"2026-03-25T06:00:01.000Z","message":{"role":"assistant","id":"msg_1","type":"message","model":"claude-3-opus","content":[{"type":"tool_use","id":"tu_1","name":"Bash","input":{"command":"npm build"}}],"stop_reason":"tool_use","usage":{"input_tokens":100,"output_tokens":50}},"isSidechain":false,"userType":"external"}"#;
        let line2 = r#"{"type":"user","uuid":"u1","timestamp":"2026-03-25T06:00:02.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","content":"Build succeeded","is_error":false}]},"isMeta":true,"isSidechain":false,"sourceToolUseId":"tu_1","toolUseResult":{"toolName":"Bash","output":"ok"}}"#;

        let session_path = project_dir.join("session-1.jsonl");
        fs::write(&session_path, format!("{}\n{}\n", line1, line2)).unwrap();

        let trigger = make_error_trigger();
        let result = test_trigger(&trigger, &scanner, None).await;

        assert_eq!(result.total_count, 0);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn test_test_trigger_empty_projects() {
        let (_temp_dir, scanner) = setup_test_env();

        let trigger = make_error_trigger();
        let result = test_trigger(&trigger, &scanner, None).await;

        assert_eq!(result.total_count, 0);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn test_test_trigger_respects_limit() {
        let (temp_dir, scanner) = setup_test_env();

        // Create multiple sessions with errors.
        for i in 0..5 {
            create_session_with_error(
                temp_dir.path().join("projects").as_path(),
                "-Users-test-project",
                &format!("session-{}", i),
                &format!("Build failed: error E{:04}", i),
            );
        }

        let trigger = make_error_trigger();
        let result = test_trigger(&trigger, &scanner, Some(2)).await;

        assert_eq!(result.errors.len(), 2);
        // total_count may be >= 2 since we count all session errors
        assert!(result.total_count >= 2);
    }

    #[tokio::test]
    async fn test_test_trigger_multiple_projects() {
        let (temp_dir, scanner) = setup_test_env();

        create_session_with_error(
            temp_dir.path().join("projects").as_path(),
            "-Users-test-project1",
            "session-1",
            "Build failed: project1",
        );

        create_session_with_error(
            temp_dir.path().join("projects").as_path(),
            "-Users-test-project2",
            "session-1",
            "Build failed: project2",
        );

        let trigger = make_error_trigger();
        let result = test_trigger(&trigger, &scanner, None).await;

        assert_eq!(result.total_count, 2);
        assert_eq!(result.errors.len(), 2);
    }

    #[tokio::test]
    async fn test_test_trigger_invalid_jsonl_skipped() {
        let (temp_dir, scanner) = setup_test_env();

        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();

        // Write invalid JSONL.
        fs::write(project_dir.join("session-bad.jsonl"), "not valid json\n{{broken").unwrap();

        // Write a valid session alongside it.
        create_session_with_error(
            temp_dir.path().join("projects").as_path(),
            "-Users-test-project",
            "session-good",
            "Valid error",
        );

        let trigger = make_error_trigger();
        let result = test_trigger(&trigger, &scanner, None).await;

        // Should still find the valid session's error.
        assert_eq!(result.total_count, 1);
    }

    // ---------------------------------------------------------------------------
    // detect_errors_with_trigger tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_detect_errors_with_trigger_empty_messages() {
        let trigger = make_error_trigger();
        let errors = detect_errors_with_trigger(
            &[],
            &trigger,
            "session-1",
            "-Users-test-project",
            "/path/session.jsonl",
        );

        assert!(errors.is_empty());
    }

    // ---------------------------------------------------------------------------
    // Helper
    // ---------------------------------------------------------------------------

    fn create_test_detected_error(id: &str) -> DetectedError {
        DetectedError {
            id: id.to_string(),
            session_id: "sess".to_string(),
            project_id: "proj".to_string(),
            file_path: "/path".to_string(),
            source: "Bash".to_string(),
            message: "error".to_string(),
            timestamp: 0,
            line_number: None,
            tool_use_id: None,
            subagent_id: None,
            trigger_color: None,
            trigger_id: None,
            trigger_name: None,
            context: crate::types::config::ErrorContext {
                project_name: "proj".to_string(),
                cwd: None,
            },
        }
    }
}
