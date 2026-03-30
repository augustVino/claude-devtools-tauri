# Search Mutex and Memory Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix std::sync::Mutex blocking the async runtime in search commands and eliminate unbounded memory allocation in `search_all_projects`.

**Architecture:** Wrap synchronous `SessionSearcher` calls in `tokio::task::spawn_blocking` to move blocking I/O off async threads (C1). Replace unbounded `Vec` accumulation with a `BinaryHeap<Reverse<SearchResult>>` capped at `max_results` (I2). `create_searcher_state` returns `Arc<Mutex<...>>` to enable `Arc::clone` into the blocking closure.

**Tech Stack:** Rust, Tauri v2, tokio, std::collections::BinaryHeap

**Spec:** `docs/superpowers/specs/2026-03-30-search-mutex-memory-fix-design.md`

**Known behavioral change:** `total_matches` in `search_all_projects` return value changes semantics. Previously it counted all matches before truncation. Now it counts the sum of per-project `total_matches` (each already bounded by `max_results`). The `is_partial` flag still works correctly for the frontend. The frontend does not currently display `total_matches` as an exact count — it only uses `is_partial` to show "more results available."

---

### Task 1: Add `Ord` trait to `SearchResult`

**Files:**
- Modify: `src-tauri/src/types/domain.rs:194-218`
- Modify: `src-tauri/src/discovery/session_searcher.rs` (test only)

- [ ] **Step 1: Write the failing test**

In `src-tauri/src/discovery/session_searcher.rs`, add at the end of the `#[cfg(test)] mod tests` block:

```rust
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
    // timestamp desc: r2 (200) > r1 (100)
    assert!(r2 > r1);
    assert!(r1 < r2);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd src-tauri && cargo test test_search_result_ordering -- --nocapture`
Expected: compile error — `binary operation `>` cannot be applied to type SearchResult`

- [ ] **Step 3: Add `Eq`, `Ord`, `PartialOrd` impls**

In `src-tauri/src/types/domain.rs`, after the `SearchResult` struct definition (after line 218):

```rust
impl Eq for SearchResult {}

impl Ord for SearchResult {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.timestamp.cmp(&other.timestamp).reverse()
    }
}

impl PartialOrd for SearchResult {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd src-tauri && cargo test test_search_result_ordering -- --nocapture`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/types/domain.rs src-tauri/src/discovery/session_searcher.rs
git commit -m "feat(search): add Ord trait to SearchResult for bounded heap"
```

---

### Task 2: Rewrite `search_all_projects` with bounded `BinaryHeap`

**Files:**
- Modify: `src-tauri/src/discovery/session_searcher.rs:142-193`

- [ ] **Step 1: Write failing tests for bounded heap behavior**

In `src-tauri/src/discovery/session_searcher.rs`, add to the `#[cfg(test)] mod tests` block:

```rust
#[test]
fn test_search_all_projects_top_n() {
    let (temp_dir, mut searcher) = setup_test_env();

    // Create 3 projects with different timestamps.
    // Each JSONL has exactly 1 user message with the unique keyword "xyzzy",
    // so each project produces exactly 1 SearchResult.
    for (project_name, ts) in [
        ("-Users-proj-a", "2024-01-01T00:00:00Z"),  // timestamp 1704067200000
        ("-Users-proj-b", "2025-06-15T12:00:00Z"),  // timestamp 1750000800000
        ("-Users-proj-c", "2023-01-01T00:00:00Z"),  // timestamp 1672531200000
    ] {
        let project_dir = temp_dir.path().join("projects").join(project_name);
        fs::create_dir_all(&project_dir).unwrap();
        let session_content = format!(
            r#"{{"type":"user","message":"unique keyword xyzzy","timestamp":"{}"}}"#,
            ts
        );
        fs::write(project_dir.join("session-1.jsonl"), session_content).unwrap();
    }

    let result = searcher.search_all_projects("xyzzy", 2);

    // Should return at most 2 results
    assert!(result.results.len() <= 2);

    // Results should be sorted by timestamp descending
    for i in 0..result.results.len().saturating_sub(1) {
        assert!(
            result.results[i].timestamp >= result.results[i + 1].timestamp,
            "Results not sorted descending: {} >= {}",
            result.results[i].timestamp,
            result.results[i + 1].timestamp
        );
    }

    // Top 2 should be proj-b (2025) and proj-a (2024), not proj-c (2023)
    assert_eq!(result.results[0].project_id, "-Users-proj-b");
    assert_eq!(result.results[1].project_id, "-Users-proj-a");
}

#[test]
fn test_search_all_projects_empty_query() {
    let (_temp_dir, mut searcher) = setup_test_env();
    let result = searcher.search_all_projects("", 50);
    assert_eq!(result.total_matches, 0);
    assert!(result.results.is_empty());
}

#[test]
fn test_search_all_projects_is_partial() {
    let (temp_dir, mut searcher) = setup_test_env();

    // Create 2 projects each with 2 matches for "alpha"
    for project_name in ["-Users-proj-x", "-Users-proj-y"] {
        let project_dir = temp_dir.path().join("projects").join(project_name);
        fs::create_dir_all(&project_dir).unwrap();
        let session_content = r#"{"type":"user","message":"alpha beta","timestamp":"2024-01-01T00:00:00Z"}
{"type":"assistant","content":[{"type":"text","text":"alpha gamma"}],"timestamp":"2024-01-01T00:01:00Z"}
"#;
        fs::write(project_dir.join("session-1.jsonl"), session_content).unwrap();
    }

    // max_results=1, but there are multiple matches across projects
    let result = searcher.search_all_projects("alpha", 1);
    assert_eq!(result.results.len(), 1);
    assert_eq!(result.is_partial, Some(true));

    // max_results=100, all results fit
    let result = searcher.search_all_projects("alpha", 100);
    assert_eq!(result.is_partial, None);
}
```

- [ ] **Step 2: Run tests to verify they fail (current impl uses unbounded Vec)**

Run: `cd src-tauri && cargo test test_search_all_projects -- --nocapture`
Expected: `test_search_all_projects_top_n` FAILS — current code returns all results in project scan order, not top-N by timestamp.

- [ ] **Step 3: Rewrite `search_all_projects` with BinaryHeap**

Replace the entire `search_all_projects` method in `session_searcher.rs` (lines 142-193) with:

```rust
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
        let mut total_matches = 0u32;
        let mut sessions_searched = 0u32;

        for project in &projects {
            let result = self.search_sessions(&project.id, query, max_results);
            total_matches += result.total_matches;
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

        SearchSessionsResult {
            results,
            total_matches,
            sessions_searched,
            query: query.to_string(),
            is_partial: if total_matches > max_results { Some(true) } else { None },
        }
    }
```

Also remove the old TODO comment at lines 146-148 (it's being fixed now).

- [ ] **Step 4: Run all search tests**

Run: `cd src-tauri && cargo test test_search -- --nocapture`
Expected: ALL PASS (existing tests + new tests)

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/discovery/session_searcher.rs
git commit -m "perf(search): use bounded BinaryHeap in search_all_projects"
```

---

### Task 3: Migrate to `Arc<Mutex<...>>` and wrap commands in `spawn_blocking`

**Files:**
- Modify: `src-tauri/src/commands/search.rs:16-64`
- Modify: `src-tauri/src/commands/http_server.rs:78`

> Note: `lib.rs` does NOT need changes. `app.manage()` accepts any `Send + Sync` type, and `Arc<Mutex<...>>` satisfies this. The `app.manage(create_searcher_state(...))` call at `lib.rs:177` works unchanged.

This task combines the return type change, http_server fix, and spawn_blocking wrapper into a single atomic commit so the code always compiles.

- [ ] **Step 1: Rewrite all of `search.rs`**

Replace the entire contents of `src-tauri/src/commands/search.rs` with:

```rust
//! IPC Handlers for Search Operations.
//!
//! Handlers:
//! - search_sessions: Search sessions in a project
//! - search_all_projects: Search sessions across all projects

use crate::discovery::SessionSearcher;
use crate::infrastructure::fs_provider::FsProvider;
use crate::types::domain::SearchSessionsResult;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::State;

/// Search sessions in a project.
#[tauri::command]
pub async fn search_sessions(
    project_id: String,
    query: String,
    max_results: Option<u32>,
    searcher: State<'_, Arc<Mutex<SessionSearcher>>>,
) -> Result<SearchSessionsResult, String> {
    let max = max_results.unwrap_or(50).min(100).max(1);

    if query.trim().is_empty() {
        return Ok(SearchSessionsResult {
            results: Vec::new(),
            total_matches: 0,
            sessions_searched: 0,
            query,
            is_partial: None,
        });
    }

    let searcher = searcher.inner().clone();
    tokio::task::spawn_blocking(move || {
        let mut s = searcher.lock().map_err(|e| e.to_string())?;
        Ok(s.search_sessions(&project_id, &query, max))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Search sessions across all projects.
#[tauri::command]
pub async fn search_all_projects(
    query: String,
    max_results: Option<u32>,
    searcher: State<'_, Arc<Mutex<SessionSearcher>>>,
) -> Result<SearchSessionsResult, String> {
    let max = max_results.unwrap_or(50).min(100).max(1);

    if query.trim().is_empty() {
        return Ok(SearchSessionsResult {
            results: Vec::new(),
            total_matches: 0,
            sessions_searched: 0,
            query,
            is_partial: None,
        });
    }

    let searcher = searcher.inner().clone();
    tokio::task::spawn_blocking(move || {
        let mut s = searcher.lock().map_err(|e| e.to_string())?;
        Ok(s.search_all_projects(&query, max))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Create a SessionSearcher state.
pub fn create_searcher_state(
    projects_dir: PathBuf,
    todos_dir: PathBuf,
    fs_provider: Arc<dyn FsProvider>,
) -> Arc<Mutex<SessionSearcher>> {
    Arc::new(Mutex::new(SessionSearcher::new(
        projects_dir,
        todos_dir,
        fs_provider,
    )))
}
```

Key changes from the original:
- `create_searcher_state` returns `Arc<Mutex<SessionSearcher>>` instead of `Mutex<SessionSearcher>`
- Both command handlers use `State<'_, Arc<Mutex<SessionSearcher>>>` and wrap the synchronous call in `spawn_blocking`
- Empty query early-return is preserved before `spawn_blocking`
- `query` is moved (not cloned) into the return struct — correct since it's not used after the early return
- `.min(100).max(1)` input validation preserved

- [ ] **Step 2: Fix `http_server.rs` double-Arc**

In `src-tauri/src/commands/http_server.rs`, line 78:

```rust
// Before:
let searcher = Arc::new(create_searcher_state(projects_dir, todos_dir, Arc::new(LocalFsProvider::new())));

// After:
let searcher = create_searcher_state(projects_dir, todos_dir, Arc::new(LocalFsProvider::new()));
```

- [ ] **Step 3: Verify full compile**

Run: `cd src-tauri && cargo check`
Expected: PASS — no errors

- [ ] **Step 4: Run all tests**

Run: `cd src-tauri && cargo test`
Expected: ALL PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands/search.rs src-tauri/src/commands/http_server.rs
git commit -m "fix(search): wrap search commands in spawn_blocking, return Arc<Mutex<...>> from factory"
```

---

### Task 4: Add TODO comment to HTTP search route

**Files:**
- Modify: `src-tauri/src/http/routes/search.rs:37`

- [ ] **Step 1: Add TODO comment**

In `src-tauri/src/http/routes/search.rs`, add a comment before the `state.searcher.lock()` call at line 37:

```rust
    // TODO: Apply spawn_blocking (see docs/superpowers/specs/2026-03-30-search-mutex-memory-fix-design.md)
    let mut searcher = state
```

- [ ] **Step 2: Verify compile**

Run: `cd src-tauri && cargo check`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/http/routes/search.rs
git commit -m "docs(search): add TODO for HTTP route spawn_blocking fix"
```

---

### Task 5: Final verification

- [ ] **Step 1: Run full test suite**

Run: `cd src-tauri && cargo test`
Expected: ALL PASS

- [ ] **Step 2: Run cargo clippy**

Run: `cd src-tauri && cargo clippy -- -D warnings`
Expected: No warnings related to our changes

- [ ] **Step 3: Verify `pnpm tauri dev` launches**

Run: `pnpm tauri dev` and test the search functionality in the app UI.
Expected: App launches, search works, no runtime errors in console.

- [ ] **Step 4: Final commit if any fixes needed**
