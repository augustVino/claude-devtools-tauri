//! SessionSearcher - Searches sessions for query strings.
//!
//! Responsibilities:
//! - Search across sessions in a project
//! - Search within a single session file
//! - Extract context around each match occurrence

use crate::discovery::project_scanner::ProjectScanner;
use crate::infrastructure::fs_provider::FsProvider;
use crate::types::domain::{SearchResult, SearchSessionsResult};
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
}

/// SessionSearcher provides methods for searching sessions.
pub struct SessionSearcher {
    projects_dir: PathBuf,
    fs_provider: Arc<dyn FsProvider>,
    // LRU cache: file_path -> CacheEntry (with mtime-based invalidation and capacity eviction)
    cache: moka::sync::Cache<String, CacheEntry>,
    /// ProjectScanner used for cross-project search.
    project_scanner: ProjectScanner,
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
    pub fn new(projects_dir: PathBuf, todos_dir: PathBuf, fs_provider: Arc<dyn FsProvider>) -> Self {
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

            let mtime = dirent.mtime_ms.unwrap_or(0);
            let path = project_path.join(&dirent.name);
            session_files.push((dirent.name, path, mtime));
        }

        // Sort by modification time (most recent first)
        session_files.sort_by(|a, b| b.2.cmp(&a.2));

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
        let entries = {
            let cache_key = file_path.to_string_lossy().to_string();
            if let Some(cached) = self.cache.get(&cache_key) {
                if cached.mtime == mtime {
                    cached.entries.clone()
                } else {
                    let entries = self.extract_searchable_entries(file_path)?;
                    self.cache.insert(cache_key, CacheEntry { mtime, entries: entries.clone() });
                    entries
                }
            } else {
                let entries = self.extract_searchable_entries(file_path)?;
                self.cache.insert(cache_key, CacheEntry { mtime, entries: entries.clone() });
                entries
            }
        };

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
            );
        }

        Ok(results)
    }

    /// Extract searchable entries from a session file.
    fn extract_searchable_entries(&self, file_path: &Path) -> Result<Vec<SearchableEntry>, std::io::Error> {
        let content = self.fs_provider.read_file(file_path)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let mut entries = Vec::new();

        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }

            // Try to parse as JSON
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                // Extract text based on message type
                let message_type = json.get("type").and_then(|t| t.as_str()).unwrap_or("unknown");

                // 提取 uuid 用于 group_id 和 message_uuid
                let uuid = json.get("uuid").and_then(|u| u.as_str()).map(|s| s.to_string());

                // 噪声过滤：跳过 sidechain 消息
                let is_sidechain = json.get("isSidechain").and_then(|v| v.as_bool()).unwrap_or(false);
                if is_sidechain {
                    continue;
                }

                // 噪声过滤：跳过 synthetic assistant 消息
                if message_type == "assistant" {
                    let model = json.get("message")
                        .and_then(|m| m.get("model"))
                        .and_then(|m| m.as_str())
                        .unwrap_or("");
                    if model == "<synthetic>" {
                        continue;
                    }
                }

                // 噪声过滤：硬噪声类型
                let hard_noise_types = ["system", "summary", "file-history-snapshot", "queue-operation"];
                if hard_noise_types.contains(&message_type) {
                    continue;
                }

                let text = match message_type {
                    "user" => {
                        // 用户消息：提取 content
                        // 噪声过滤：跳过仅含噪声标签的用户消息
                        let raw = json.get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("");
                        if is_noise_only_user_message(raw) {
                            String::new()
                        } else {
                            raw.to_string()
                        }
                    }
                    "assistant" => {
                        // Assistant 消息：仅使用最后一个文本输出块（与 Electron 对齐）
                        if let Some(content) = json.get("content") {
                            if let Some(arr) = content.as_array() {
                                // 反向查找最后一个 text 类型的块
                                arr.iter()
                                    .rev()
                                    .filter_map(|item| {
                                        if item.get("type")?.as_str()? == "text" {
                                            item.get("text").and_then(|t| t.as_str())
                                        } else {
                                            None
                                        }
                                    })
                                    .next()
                                    .unwrap_or("")
                                    .to_string()
                            } else {
                                String::new()
                            }
                        } else {
                            String::new()
                        }
                    }
                    _ => String::new(),
                };

                let timestamp = json.get("timestamp")
                    .and_then(|t| t.as_str())
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                    .map(|dt| dt.timestamp_millis() as u64)
                    .unwrap_or(0);

                if !text.is_empty() {
                    // 构建 group_id 和 item_type（与 Electron 的 chunk ID 格式对齐）
                    // 注意：assistant 消息的 group_id 使用 "ai-" 前缀（与 chunk_builder 一致），
                    // 而非 message_type 中的 "assistant"，前端 DOM 使用 data-search-item-id="ai-{uuid}"
                    let (group_id, item_type) = if let Some(ref id) = uuid {
                        let group = match message_type {
                            "assistant" => format!("ai-{}", id),
                            "user" => format!("user-{}", id),
                            _ => format!("{}-{}", message_type, id),
                        };
                        let itype = match message_type {
                            "user" => Some("user".to_string()),
                            "assistant" => Some("ai".to_string()),
                            _ => None,
                        };
                        (Some(group), itype)
                    } else {
                        (None, None)
                    };

                    entries.push(SearchableEntry {
                        text,
                        message_type: message_type.to_string(),
                        timestamp,
                        group_id,
                        item_type,
                        message_uuid: uuid,
                    });
                }
            }
        }

        Ok(entries)
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
                session_title: "Untitled Session".to_string(),
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

/// 检查用户消息是否仅包含噪声标签（与 Electron 的 `isParsedHardNoiseMessage` 对齐）。
///
/// Electron 的检测逻辑：检查消息是否被噪声标签完整包裹（以 `<tag>` 开头，以 `</tag>` 结尾），
/// 而非剥离所有标签后检查内容。这确保了包含噪声标签但同时也包含用户文本的消息不被误过滤。
fn is_noise_only_user_message(text: &str) -> bool {
    let trimmed = text.trim();

    if trimmed.is_empty() {
        return true;
    }

    // 检查噪声标签包裹（与 Electron HARD_NOISE_TAGS 对齐）
    const NOISE_TAGS: &[&str] = &["<local-command-caveat>", "<system-reminder>"];

    for &tag in NOISE_TAGS {
        let close_tag = tag.replacen('<', "</", 1);
        if trimmed.starts_with(tag) && trimmed.ends_with(&close_tag) {
            return true;
        }
    }

    // 过滤中断消息（与 Electron 对齐）
    if trimmed.starts_with("[Request interrupted by user") {
        return true;
    }

    false
}

/// 构建 SSH 快速搜索的分阶段边界索引（与 Electron 对齐）。
///
/// 边界是会话文件列表中的位置，到达这些位置时如果已收集足够结果则提前退出。
fn build_fast_search_stage_boundaries(total_files: usize) -> Vec<usize> {
    SSH_FAST_SEARCH_STAGE_LIMITS
        .iter()
        .filter(|&&limit| limit < total_files)
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::fs_provider::LocalFsProvider;
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;

    // ---------------------------------------------------------------------------
    // is_noise_only_user_message tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_noise_filter_empty() {
        assert!(is_noise_only_user_message(""));
        assert!(is_noise_only_user_message("   "));
    }

    #[test]
    fn test_noise_filter_wrapped_system_reminder() {
        assert!(is_noise_only_user_message("<system-reminder>rules here</system-reminder>"));
    }

    #[test]
    fn test_noise_filter_wrapped_local_command_caveat() {
        assert!(is_noise_only_user_message("<local-command-caveat>some caveat</local-command-caveat>"));
    }

    #[test]
    fn test_noise_filter_mixed_content_not_filtered() {
        // 包含噪声标签但同时也包含用户文本 → 不应过滤
        assert!(!is_noise_only_user_message(
            "<system-reminder>rules</system-reminder>Please help me with this"
        ));
    }

    #[test]
    fn test_noise_filter_normal_user_text_not_filtered() {
        assert!(!is_noise_only_user_message("Hello, can you help me?"));
    }

    #[test]
    fn test_noise_filter_interruption_message() {
        assert!(is_noise_only_user_message("[Request interrupted by user at 2024-01-01]"));
    }

    #[test]
    fn test_noise_filter_tag_with_extra_content_after_close() {
        // 关闭标签后有额外内容 → 不应过滤
        assert!(!is_noise_only_user_message(
            "<system-reminder>rules</system-reminder>extra text"
        ));
    }

    // ---------------------------------------------------------------------------
    // setup helpers
    // ---------------------------------------------------------------------------

    fn setup_test_env() -> (TempDir, SessionSearcher) {
        let temp_dir = TempDir::new().unwrap();
        let projects_dir = temp_dir.path().join("projects");
        let todos_dir = temp_dir.path().join("todos");
        fs::create_dir_all(&projects_dir).unwrap();
        fs::create_dir_all(&todos_dir).unwrap();

        let searcher = SessionSearcher::new(projects_dir, todos_dir, Arc::new(LocalFsProvider::new()));
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

        // Create a project directory with a session file
        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();

        let session_content = r#"{"type":"user","message":"Hello world this is a test","timestamp":"2024-01-01T00:00:00Z"}
{"type":"assistant","content":[{"type":"text","text":"This is the assistant response with test keyword"}],"timestamp":"2024-01-01T00:01:00Z"}
"#;
        fs::write(project_dir.join("session-123.jsonl"), session_content).unwrap();

        let result = searcher.search_sessions("-Users-test-project", "test", 50);
        assert!(result.total_matches > 0);
        assert_eq!(result.sessions_searched, 1);
    }

    #[test]
    fn test_search_max_results() {
        let (temp_dir, mut searcher) = setup_test_env();

        // Create a project directory with a session file
        let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
        fs::create_dir_all(&project_dir).unwrap();

        // Create content with multiple occurrences
        let session_content = r#"{"type":"user","message":"test test test test test","timestamp":"2024-01-01T00:00:00Z"}
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
}
