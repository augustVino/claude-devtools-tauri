//! SessionSearcher - Searches sessions for query strings.
//!
//! Responsibilities:
//! - Search across sessions in a project
//! - Search within a single session file
//! - Extract context around each match occurrence

use crate::discovery::project_scanner::ProjectScanner;
use crate::discovery::SubprojectRegistry;
use crate::infrastructure::fs_provider::FsProvider;
use crate::parsing::jsonl_parser::{deduplicate_by_request_id, extract_text_content, parse_jsonl_content};
use crate::parsing::message_classifier::{classify_messages, group_ai_messages};
use crate::types::domain::{GroupedMessage, MessageCategory, SearchResult, SearchSessionsResult};
use crate::utils::content_sanitizer::{extract_session_title_from_parsed, sanitize_display_content};
use crate::utils::path_decoder;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// SSH 快速搜索时间预算（毫秒），与 Electron 对齐。
const SSH_FAST_SEARCH_TIME_BUDGET_MS: u128 = 4500;

/// SSH 快速搜索分阶段边界，与 Electron 对齐。
const SSH_FAST_SEARCH_STAGE_LIMITS: &[usize] = &[40, 140, 320];

/// SSH 快速搜索提前退出的最小结果数。
const SSH_FAST_SEARCH_MIN_RESULTS: usize = 8;

/// 搜索缓存最大容量（与 Electron 的 SearchTextCache 对齐）。
const CACHE_MAX_CAPACITY: u64 = 1000;

/// 搜索缓存条目。
#[derive(Debug, Clone)]
struct CacheEntry {
    mtime: u64,
    entries: Vec<SearchableEntry>,
    session_title: Option<String>,
}

/// SessionSearcher provides methods for searching sessions.
pub struct SessionSearcher {
    projects_dir: PathBuf,
    fs_provider: Arc<dyn FsProvider>,
    // LRU cache: file_path -> CacheEntry (with mtime-based invalidation and capacity eviction)
    cache: moka::sync::Cache<String, CacheEntry>,
    /// ProjectScanner used for cross-project search.
    project_scanner: ProjectScanner,
    /// Optional subproject registry for session filtering by composite project ID.
    subproject_registry: Option<Arc<std::sync::Mutex<SubprojectRegistry>>>,
}

/// A searchable entry extracted from a message.
#[derive(Debug, Clone)]
struct SearchableEntry {
    text: String,
    message_type: String,
    timestamp: u64,
    group_id: Option<String>,
    item_type: Option<String>,
    message_uuid: Option<String>,
}

impl SessionSearcher {
    /// Create a new SessionSearcher.
    pub fn new(
        projects_dir: PathBuf,
        todos_dir: PathBuf,
        fs_provider: Arc<dyn FsProvider>,
        subproject_registry: Option<Arc<std::sync::Mutex<SubprojectRegistry>>>,
    ) -> Self {
        let project_scanner = ProjectScanner::with_paths(
            projects_dir.clone(),
            todos_dir,
            fs_provider.clone(),
        );
        Self {
            projects_dir,
            fs_provider,
            cache: moka::sync::Cache::builder()
                .max_capacity(CACHE_MAX_CAPACITY)
                .build(),
            project_scanner,
            subproject_registry,
        }
    }

    /// Search sessions in a project for a query string.
    pub fn search_sessions(
        &mut self,
        project_id: &str,
        query: &str,
        max_results: u32,
    ) -> SearchSessionsResult {
        if query.trim().is_empty() {
            return SearchSessionsResult {
                results: Vec::new(),
                total_matches: 0,
                sessions_searched: 0,
                query: query.to_string(),
                is_partial: None,
            };
        }

        let normalized_query = query.trim().to_lowercase();
        let mut results: Vec<SearchResult> = Vec::new();
        let mut sessions_searched: u32 = 0;
        let mut is_partial = false;

        // SSH 快速搜索模式：时间预算 + 分阶段边界
        let fast_mode = self.fs_provider.provider_type() == "ssh";
        let started_at = Instant::now();

        let base_dir = path_decoder::extract_base_dir(project_id);
        let project_path = self.projects_dir.join(&base_dir);

        if !self.fs_provider.exists(&project_path).unwrap_or(false) {
            return SearchSessionsResult {
                results: Vec::new(),
                total_matches: 0,
                sessions_searched: 0,
                query: query.to_string(),
                is_partial: None,
            };
        }

        // Get all session files
        let entries = match self.fs_provider.read_dir(&project_path) {
            Ok(entries) => entries,
            Err(_) => return SearchSessionsResult {
                results: Vec::new(),
                total_matches: 0,
                sessions_searched: 0,
                query: query.to_string(),
                is_partial: None,
            },
        };

        let mut session_files: Vec<(String, PathBuf, u64)> = Vec::new();

        for dirent in entries {
            if !dirent.is_file || !dirent.name.ends_with(".jsonl") {
                continue;
            }

            let path = project_path.join(&dirent.name);
            let mtime = match dirent.mtime_ms {
                Some(ms) => ms,
                None => self.fs_provider.stat(&path).map(|m| m.mtime_ms).unwrap_or(0),
            };
            session_files.push((dirent.name, path, mtime));
        }

        // Sort by modification time (most recent first)
        session_files.sort_by(|a, b| b.2.cmp(&a.2));

        // Resolve subproject session filter if a registry is available
        let session_filter = self.subproject_registry
            .as_ref()
            .and_then(|reg| reg.lock().ok())
            .and_then(|guard| guard.get_session_filter(project_id).cloned());

        // 构建分阶段搜索边界（SSH fast search 使用）
        let stage_boundaries = if fast_mode {
            build_fast_search_stage_boundaries(session_files.len())
        } else {
            Vec::new()
        };

        // Search each session file
        for (idx, (file_name, file_path, mtime)) in session_files.iter().enumerate() {
            if results.len() >= max_results as usize {
                break;
            }

            // SSH 时间预算检查
            if fast_mode && started_at.elapsed().as_millis() >= SSH_FAST_SEARCH_TIME_BUDGET_MS {
                is_partial = true;
                break;
            }

            // SSH 分阶段边界检查：若已收集足够结果且到达阶段边界则提前退出
            if fast_mode {
                if let Some(&boundary) = stage_boundaries.iter().find(|&&b| b == idx) {
                    if results.len() >= SSH_FAST_SEARCH_MIN_RESULTS {
                        is_partial = true;
                        break;
                    }
                }
            }

            let session_id = path_decoder::extract_session_id(file_name);

            // Skip sessions not belonging to this subproject
            if let Some(ref filter) = session_filter {
                if !filter.contains(&session_id) {
                    continue;
                }
            }

            sessions_searched += 1;

            if let Ok(file_results) = self.search_session_file(
                project_id,
                &session_id,
                file_path,
                &normalized_query,
                max_results as usize - results.len(),
                *mtime,
            ) {
                results.extend(file_results);
            }
        }

        // SSH 模式下：若因 results 达到上限退出但仍有未搜索的会话，标记为 partial
        if fast_mode && !is_partial
            && results.len() >= max_results as usize
            && (sessions_searched as usize) < session_files.len()
        {
            is_partial = true;
        }

        let total_matches = results.len() as u32;
        SearchSessionsResult {
            results,
            total_matches,
            sessions_searched,
            query: query.to_string(),
            is_partial: if is_partial { Some(true) } else { None },
        }
    }

    /// Search sessions across all projects.
    /// Uses a bounded BinaryHeap to maintain only the top `max_results` entries
    /// by timestamp, keeping memory at O(max_results) instead of O(total_matches).
    pub fn search_all_projects(
        &mut self,
        query: &str,
        max_results: u32,
    ) -> SearchSessionsResult {
        let projects = self.project_scanner.scan();

        if projects.is_empty() || query.trim().is_empty() {
            return SearchSessionsResult {
                results: Vec::new(),
                total_matches: 0,
                sessions_searched: 0,
                query: query.to_string(),
                is_partial: None,
            };
        }

        use std::cmp::Reverse;
        use std::collections::BinaryHeap;

        let capacity = max_results as usize;
        let mut heap: BinaryHeap<Reverse<SearchResult>> = BinaryHeap::with_capacity(capacity);
        let mut matches_found = 0u32;
        let mut sessions_searched = 0u32;

        for project in &projects {
            let result = self.search_sessions(&project.id, query, max_results);
            matches_found += result.total_matches;
            sessions_searched += result.sessions_searched;

            for r in result.results {
                if heap.len() < capacity {
                    heap.push(Reverse(r));
                } else if r.timestamp > heap.peek().unwrap().0.timestamp {
                    heap.pop();
                    heap.push(Reverse(r));
                }
            }
        }

        let mut results: Vec<SearchResult> = heap.into_iter().map(|Reverse(r)| r).collect();
        results.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        let results_len = results.len() as u32;
        SearchSessionsResult {
            results,
            total_matches: matches_found,
            sessions_searched,
            query: query.to_string(),
            is_partial: if matches_found > results_len {
                Some(true)
            } else {
                None
            },
        }
    }

    /// Search a single session file for a query string.
    fn search_session_file(
        &mut self,
        project_id: &str,
        session_id: &str,
        file_path: &Path,
        query: &str,
        max_results: usize,
        mtime: u64,
    ) -> Result<Vec<SearchResult>, std::io::Error> {
        let mut results = Vec::new();

        // Check cache (LRU with mtime-based invalidation)
        let (entries, session_title) = {
            let cache_key = file_path.to_string_lossy().to_string();
            if let Some(cached) = self.cache.get(&cache_key) {
                if cached.mtime == mtime {
                    (cached.entries.clone(), cached.session_title.clone())
                } else {
                    let (entries, session_title) = self.extract_searchable_entries(file_path)?;
                    self.cache.insert(cache_key, CacheEntry { mtime, entries: entries.clone(), session_title: session_title.clone() });
                    (entries, session_title)
                }
            } else {
                let (entries, session_title) = self.extract_searchable_entries(file_path)?;
                self.cache.insert(cache_key, CacheEntry { mtime, entries: entries.clone(), session_title: session_title.clone() });
                (entries, session_title)
            }
        };

        let title_str = session_title.as_deref().unwrap_or("Untitled Session");

        // Fast pre-filter: skip sessions where no entry contains the query
        let has_any_match = entries.iter().any(|e| e.text.to_lowercase().contains(query));
        if !has_any_match {
            return Ok(results);
        }

        // Collect matches
        for entry in &entries {
            if results.len() >= max_results {
                break;
            }

            self.collect_matches_for_entry(
                entry,
                query,
                &mut results,
                max_results,
                project_id,
                session_id,
                title_str,
            );
        }

        Ok(results)
    }

    /// Extract searchable entries from a session file.
    ///
    /// Uses the full parsing pipeline:
    /// read file -> parse_jsonl_content() -> deduplicate_by_request_id() ->
    /// classify_messages() -> group_ai_messages() -> generate SearchableEntry
    fn extract_searchable_entries(&self, file_path: &Path) -> Result<(Vec<SearchableEntry>, Option<String>), std::io::Error> {
        let content = self.fs_provider.read_file(file_path)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        let parsed = parse_jsonl_content(&content);
        let deduped = deduplicate_by_request_id(&parsed);
        let classified = classify_messages(&deduped);
        let grouped = group_ai_messages(classified);

        let mut entries = Vec::new();

        for gm in &grouped {
            match gm {
                GroupedMessage::Single { category: MessageCategory::User, message } => {
                    let raw_text = extract_text_content(message);
                    let text = sanitize_display_content(&raw_text);
                    if text.is_empty() {
                        continue;
                    }
                    entries.push(SearchableEntry {
                        text,
                        message_type: "user".to_string(),
                        timestamp: parse_timestamp(&message.timestamp),
                        group_id: Some(format!("user-{}", message.uuid)),
                        item_type: Some("user".to_string()),
                        message_uuid: Some(message.uuid.clone()),
                    });
                }
                GroupedMessage::AiGroup { messages, group_id } => {
                    // AI 文本提取 — 对齐 Electron: 仅取最后一条 assistant message
                    // 的最后一个 text content block（反向扫描）
                    let mut extracted_text: Option<String> = None;
                    for i in (0..messages.len()).rev() {
                        let msg = &messages[i];
                        if msg.role.as_deref() != Some("assistant") || !msg.content.is_array() {
                            continue;
                        }
                        if let Some(content_arr) = msg.content.as_array() {
                            for j in (0..content_arr.len()).rev() {
                                let block = &content_arr[j];
                                if block.get("type").and_then(|v| v.as_str()) == Some("text") {
                                    if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                        if !text.is_empty() {
                                            extracted_text = Some(text.to_string());
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        if extracted_text.is_some() {
                            break;
                        }
                    }

                    let text = match extracted_text {
                        Some(t) => t,
                        None => continue,
                    };

                    let timestamp = messages
                        .first()
                        .map(|m| parse_timestamp(&m.timestamp))
                        .unwrap_or(0);
                    let uuid = messages.first().map(|m| m.uuid.clone());
                    entries.push(SearchableEntry {
                        text,
                        message_type: "assistant".to_string(),
                        timestamp,
                        group_id: Some(group_id.clone()),
                        item_type: Some("ai".to_string()),
                        message_uuid: uuid,
                    });
                }
                // Skip HardNoise, System, Compact categories
                _ => {}
            }
        }

        let session_title = extract_session_title_from_parsed(&deduped);

        Ok((entries, session_title))
    }

    /// Collect matches from an entry.
    fn collect_matches_for_entry(
        &self,
        entry: &SearchableEntry,
        query: &str,
        results: &mut Vec<SearchResult>,
        max_results: usize,
        project_id: &str,
        session_id: &str,
        session_title: &str,
    ) {
        let lower_text = entry.text.to_lowercase();
        let mut pos = 0;
        let mut match_index_in_item: u32 = 0;

        while let Some(found_pos) = lower_text[pos..].find(query) {
            if results.len() >= max_results {
                return;
            }

            let absolute_pos = pos + found_pos;
            let context_start = absolute_pos.saturating_sub(50);
            let context_end = (absolute_pos + query.len() + 50).min(entry.text.len());

            let context = &entry.text[context_start..context_end];
            let matched_text = &entry.text[absolute_pos..absolute_pos + query.len()];

            let context_with_ellipsis = format!(
                "{}{}{}",
                if context_start > 0 { "..." } else { "" },
                context,
                if context_end < entry.text.len() { "..." } else { "" }
            );

            results.push(SearchResult {
                session_id: session_id.to_string(),
                project_id: project_id.to_string(),
                session_title: session_title.to_string(),
                matched_text: matched_text.to_string(),
                context: context_with_ellipsis,
                message_type: entry.message_type.clone(),
                timestamp: entry.timestamp,
                group_id: entry.group_id.clone(),
                item_type: entry.item_type.clone(),
                match_index_in_item: Some(match_index_in_item),
                match_start_offset: Some(absolute_pos as u32),
                message_uuid: entry.message_uuid.clone(),
            });

            match_index_in_item += 1;
            pos = absolute_pos + query.len();
        }
    }
}

/// Parse an RFC 3339 timestamp string to milliseconds since epoch.
fn parse_timestamp(ts: &str) -> u64 {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|dt| dt.timestamp_millis() as u64)
        .unwrap_or(0)
}

/// 构建 SSH 快速搜索的分阶段边界索引（与 Electron 对齐）。
///
/// 边界是会话文件列表中的位置，到达这些位置时如果已收集足够结果则提前退出。
fn build_fast_search_stage_boundaries(total_files: usize) -> Vec<usize> {
    let mut boundaries: Vec<usize> = SSH_FAST_SEARCH_STAGE_LIMITS
        .iter()
        .map(|&limit| limit.min(total_files))
        .filter(|&boundary| boundary > 0 && boundary < total_files)
        .collect();
    boundaries.dedup();
    if boundaries.last() != Some(&total_files) {
        boundaries.push(total_files);
    }
    boundaries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::fs_provider::LocalFsProvider;
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;

    // ---------------------------------------------------------------------------
    // setup helpers
    // ---------------------------------------------------------------------------

    fn setup_test_env() -> (TempDir, SessionSearcher) {
        let temp_dir = TempDir::new().unwrap();
        let projects_dir = temp_dir.path().join("projects");
        let todos_dir = temp_dir.path().join("todos");
        fs::create_dir_all(&projects_dir).unwrap();
        fs::create_dir_all(&todos_dir).unwrap();

        let searcher = SessionSearcher::new(projects_dir, todos_dir, Arc::new(LocalFsProvider::new()), None);
        (temp_dir, searcher)
    }

    #[test]
    fn test_search_empty_query() {
        let (_temp_dir, mut searcher) = setup_test_env();
        let result = searcher.search_sessions("-Users-test-project", "", 50);
        assert_eq!(result.total_matches, 0);
    }

    #[test]
    fn test_search_no_sessions() {
        let (_temp_dir, mut searcher) = setup_test_env();
        let result = searcher.search_sessions("-Users-test-project", "test", 50);
        assert_eq!(result.total_matches, 0);
    }

    #[test]
    fn test_search_with_match() {
        let (temp_dir, mut searcher) = setup_test_env();

        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();

        let session_content = r#"{"type":"user","message":{"role":"user","content":"Hello world this is a test"},"uuid":"u1","timestamp":"2024-01-01T00:00:00Z"}
{"type":"assistant","message":{"role":"assistant","id":"msg_1","type":"message","content":[{"type":"text","text":"This is the assistant response with test keyword"}],"model":"claude-3","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":20}},"uuid":"a1","timestamp":"2024-01-01T00:01:00Z"}
"#;
        fs::write(project_dir.join("session-123.jsonl"), session_content).unwrap();

        let result = searcher.search_sessions("-Users-test-project", "test", 50);
        assert!(result.total_matches > 0);
        assert_eq!(result.sessions_searched, 1);
    }

    #[test]
    fn test_search_max_results() {
        let (temp_dir, mut searcher) = setup_test_env();

        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();

        let session_content = r#"{"type":"user","message":{"role":"user","content":"test test test test test"},"uuid":"u1","timestamp":"2024-01-01T00:00:00Z"}
"#;
        fs::write(project_dir.join("session-123.jsonl"), session_content).unwrap();

        let result = searcher.search_sessions("-Users-test-project", "test", 2);
        assert!(result.results.len() <= 2);
    }

    #[test]
    fn test_search_result_ordering() {
        let r1 = SearchResult {
            session_id: "s1".into(),
            project_id: "p1".into(),
            session_title: "t1".into(),
            matched_text: "a".into(),
            context: "c".into(),
            message_type: "user".into(),
            timestamp: 100,
            group_id: None,
            item_type: None,
            match_index_in_item: None,
            match_start_offset: None,
            message_uuid: None,
        };
        let r2 = SearchResult {
            session_id: "s2".into(),
            project_id: "p2".into(),
            session_title: "t2".into(),
            matched_text: "b".into(),
            context: "d".into(),
            message_type: "user".into(),
            timestamp: 200,
            group_id: None,
            item_type: None,
            match_index_in_item: None,
            match_start_offset: None,
            message_uuid: None,
        };
        // Ord: ascending by timestamp, so r2 (200) > r1 (100)
        assert!(r2 > r1);
        assert!(r1 < r2);
    }

    // ---------------------------------------------------------------------------
    // New tests: AI buffer grouping, isMeta filtering, sanitization, session title
    // ---------------------------------------------------------------------------

    #[test]
    fn test_search_ai_buffer_grouping() {
        let (temp_dir, mut searcher) = setup_test_env();

        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();

        // 3 consecutive assistant messages with different requestIds should merge into 1 AiGroup.
        // Using distinct requestIds prevents deduplication from collapsing them.
        // New behavior (aligned with Electron): only the LAST assistant message's last text
        // content block is extracted, so the search keyword must be in the last message.
        let session_content = r#"{"type":"user","message":{"role":"user","content":"please respond"},"uuid":"u1","timestamp":"2024-01-01T00:00:00Z"}
{"type":"assistant","message":{"role":"assistant","id":"msg_1","type":"message","content":[{"type":"text","text":"First response part"}],"model":"claude-3","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":20}},"requestId":"r1","uuid":"a1","timestamp":"2024-01-01T00:00:01Z"}
{"type":"assistant","message":{"role":"assistant","id":"msg_2","type":"message","content":[{"type":"text","text":"Second response part"}],"model":"claude-3","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":20}},"requestId":"r2","uuid":"a2","timestamp":"2024-01-01T00:00:02Z"}
{"type":"assistant","message":{"role":"assistant","id":"msg_3","type":"message","content":[{"type":"text","text":"Third response part with keyword match"}],"model":"claude-3","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":20}},"requestId":"r3","uuid":"a3","timestamp":"2024-01-01T00:00:03Z"}
"#;
        fs::write(project_dir.join("session-grouping.jsonl"), session_content).unwrap();

        let result = searcher.search_sessions("-Users-test-project", "keyword", 50);
        assert_eq!(result.total_matches, 1, "should find exactly 1 match in the merged AI group");
        let sr = &result.results[0];
        assert_eq!(sr.group_id.as_deref(), Some("ai-a1"), "group_id should be ai-a1 (first assistant uuid)");
        assert_eq!(sr.item_type.as_deref(), Some("ai"));
    }

    #[test]
    fn test_search_ismeta_filtered() {
        let (temp_dir, mut searcher) = setup_test_env();

        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();

        // isMeta=true user messages should NOT appear as "user" item_type in search results.
        // They are classified as Ai (not User), so item_type should be "ai" rather than "user".
        let session_content = r#"{"type":"user","message":{"role":"user","content":"This is a meta message with unique_keyword_xyz"},"uuid":"u1","timestamp":"2024-01-01T00:00:00Z","isMeta":true}
{"type":"user","message":{"role":"user","content":"Normal user message"},"uuid":"u2","timestamp":"2024-01-01T00:01:00Z","isMeta":false}
"#;
        fs::write(project_dir.join("session-meta.jsonl"), session_content).unwrap();

        let result = searcher.search_sessions("-Users-test-project", "unique_keyword_xyz", 50);
        // The meta message is classified as Ai (not User), so it appears with item_type="ai", not "user"
        if result.total_matches > 0 {
            let sr = &result.results[0];
            assert_ne!(sr.item_type.as_deref(), Some("user"),
                "isMeta=true messages must not have item_type=user");
        }
    }

    #[test]
    fn test_search_sanitized_content() {
        let (temp_dir, mut searcher) = setup_test_env();

        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();

        // User message with noise tags should be cleaned before searching
        let session_content = r#"{"type":"user","message":{"role":"user","content":"<system-reminder>internal rules here</system-reminder>Please fix the bug"},"uuid":"u1","timestamp":"2024-01-01T00:00:00Z"}
"#;
        fs::write(project_dir.join("session-sanitize.jsonl"), session_content).unwrap();

        let result = searcher.search_sessions("-Users-test-project", "bug", 50);
        assert!(result.total_matches > 0, "should find 'bug' in sanitized content");
        let sr = &result.results[0];
        assert!(!sr.context.contains("<system-reminder>"), "context should not contain noise tags");
        assert!(sr.context.contains("bug"), "context should contain the actual search term");
    }

    #[test]
    fn test_search_session_title_extracted() {
        let (temp_dir, mut searcher) = setup_test_env();

        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();

        let session_content = r#"{"type":"user","message":{"role":"user","content":"Help me implement authentication feature"},"uuid":"u1","timestamp":"2024-01-01T00:00:00Z"}
{"type":"assistant","message":{"role":"assistant","id":"msg_1","type":"message","content":[{"type":"text","text":"Sure, I will help with authentication"}],"model":"claude-3","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":20}},"uuid":"a1","timestamp":"2024-01-01T00:01:00Z"}
"#;
        fs::write(project_dir.join("session-title.jsonl"), session_content).unwrap();

        let result = searcher.search_sessions("-Users-test-project", "authentication", 50);
        assert!(result.total_matches > 0);
        let sr = &result.results[0];
        assert_ne!(sr.session_title, "Untitled Session", "session_title should be extracted from first user message");
        assert!(sr.session_title.contains("Help me implement authentication feature"));
    }

    // ---------------------------------------------------------------------------
    // Tests: build_fast_search_stage_boundaries
    // ---------------------------------------------------------------------------

    #[test]
    fn test_fast_search_stage_boundaries_includes_final() {
        // total_files = 50, SSH_FAST_SEARCH_STAGE_LIMITS = [40, 140, 320]
        // 期望: [40, 50] (min capping + final push)
        let boundaries = build_fast_search_stage_boundaries(50);
        assert_eq!(boundaries, vec![40, 50]);
    }

    #[test]
    fn test_fast_search_stage_boundaries_large() {
        // total_files = 500
        // 期望: [40, 140, 320, 500]
        let boundaries = build_fast_search_stage_boundaries(500);
        assert_eq!(boundaries, vec![40, 140, 320, 500]);
    }

    #[test]
    fn test_fast_search_stage_boundaries_small() {
        // total_files = 10
        // 期望: [10] (所有 limits >= total_files, 只有 final push)
        let boundaries = build_fast_search_stage_boundaries(10);
        assert_eq!(boundaries, vec![10]);
    }
}
