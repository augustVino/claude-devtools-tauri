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
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// SessionSearcher provides methods for searching sessions.
pub struct SessionSearcher {
    projects_dir: PathBuf,
    fs_provider: Arc<dyn FsProvider>,
    // Simple cache: file_path -> (mtime, entries)
    cache: HashMap<String, (u64, Vec<SearchableEntry>)>,
    /// ProjectScanner used for cross-project search.
    project_scanner: ProjectScanner,
}

/// A searchable entry extracted from a message.
#[derive(Debug, Clone)]
struct SearchableEntry {
    text: String,
    message_type: String,
    timestamp: u64,
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
            cache: HashMap::new(),
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

        let normalized_query = query.to_lowercase();
        let mut results: Vec<SearchResult> = Vec::new();
        let mut sessions_searched: u32 = 0;

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

        // Search each session file
        for (file_name, file_path, mtime) in session_files {
            if results.len() >= max_results as usize {
                break;
            }

            let session_id = path_decoder::extract_session_id(&file_name);
            sessions_searched += 1;

            if let Ok(file_results) = self.search_session_file(
                project_id,
                &session_id,
                &file_path,
                &normalized_query,
                max_results as usize - results.len(),
                mtime,
            ) {
                results.extend(file_results);
            }
        }

        let total_matches = results.len() as u32;
        SearchSessionsResult {
            results,
            total_matches,
            sessions_searched,
            query: query.to_string(),
            is_partial: None,
        }
    }

    /// Search sessions across all projects.
    /// Iterates projects in batches of 8, collects all results,
    /// sorts by timestamp descending, and limits to max_results.
    ///
    /// TODO: Memory is O(total_matches) — should use BinaryHeap<Reverse<SearchResult>>
    /// to maintain only top `max_results` entries, reducing to O(max_results).
    /// Requires implementing `Ord` on `SearchResult` (sort by timestamp desc).
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

        let batch_size = 8usize;
        let mut all_results: Vec<SearchResult> = Vec::new();
        let mut sessions_searched = 0u32;

        for chunk in projects.chunks(batch_size) {
            let batch_results: Vec<SearchSessionsResult> = chunk
                .iter()
                .map(|project| self.search_sessions(&project.id, query, max_results))
                .collect();

            for result in batch_results {
                sessions_searched += result.sessions_searched;
                all_results.extend(result.results);
            }
        }

        all_results.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        let total_matches = all_results.len() as u32;
        let limited: Vec<SearchResult> = all_results.into_iter().take(max_results as usize).collect();

        SearchSessionsResult {
            results: limited,
            total_matches,
            sessions_searched,
            query: query.to_string(),
            is_partial: if total_matches > max_results { Some(true) } else { None },
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

        // Check cache
        let entries = {
            let cache_key = file_path.to_string_lossy().to_string();
            if let Some((cached_mtime, cached_entries)) = self.cache.get(&cache_key) {
                if *cached_mtime == mtime {
                    cached_entries.clone()
                } else {
                    let entries = self.extract_searchable_entries(file_path)?;
                    self.cache.insert(cache_key, (mtime, entries.clone()));
                    entries
                }
            } else {
                let entries = self.extract_searchable_entries(file_path)?;
                self.cache.insert(cache_key, (mtime, entries.clone()));
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

                let text = match message_type {
                    "user" => {
                        // User message - extract content
                        json.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string()
                    }
                    "assistant" => {
                        // Assistant message - extract text content
                        if let Some(content) = json.get("content") {
                            if let Some(arr) = content.as_array() {
                                arr.iter()
                                    .filter_map(|item| {
                                        if item.get("type")?.as_str()? == "text" {
                                            item.get("text").and_then(|t| t.as_str())
                                        } else {
                                            None
                                        }
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n")
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
                    entries.push(SearchableEntry {
                        text,
                        message_type: message_type.to_string(),
                        timestamp,
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
                group_id: None,
                item_type: None,
                match_index_in_item: None,
                match_start_offset: Some(absolute_pos as u32),
                message_uuid: None,
            });

            pos = absolute_pos + query.len();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::fs_provider::LocalFsProvider;
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;

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
}