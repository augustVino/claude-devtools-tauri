# Search Mutex and Memory Fix Design

> Date: 2026-03-30
> Issue: `docs/search-mutex-and-memory-issues.md`
> Scope: C1 (Mutex blocking async runtime) + I2 (unbounded memory in `search_all_projects`)

---

## Problem

1. **C1**: `search_sessions` and `search_all_projects` Tauri IPC handlers use `std::sync::Mutex::lock()` directly inside `async fn`. The lock is held while performing synchronous filesystem I/O and CPU-intensive string matching, blocking the tokio runtime thread for the entire duration. With many projects or slow filesystems, this can stall other IPC commands for seconds.

2. **I2**: `search_all_projects` accumulates all matches into an unbounded `Vec<SearchResult>`, sorts the full vector, then truncates to `max_results`. With N projects each returning up to 50 results, peak memory is O(N × 50) when only 50 entries are needed.

## Approach

**`spawn_blocking` + bounded priority queue** (Option A from brainstorming).

- C1: Wrap synchronous search calls in `tokio::task::spawn_blocking` to move blocking work off the async runtime threads. This follows the existing pattern in `http/routes/utility.rs`.
- I2: Replace the unbounded `Vec` with a `BinaryHeap<Reverse<SearchResult>>` capped at `max_results`, reducing memory from O(N × max) to O(max).

Both fixes are confined to the same call chain, making a single coherent change.

## Design

### C1 Fix: `spawn_blocking` wrapper

**Files changed:**

- `src-tauri/src/commands/search.rs`
- `src-tauri/src/commands/http_server.rs` — remove outer `Arc::new()` wrapper (see below)
- `src-tauri/src/http/state.rs` (already uses `Arc<Mutex<...>>`, no change needed)

**Import changes in `search.rs`**: The `use std::sync::Mutex` import is still needed — `Mutex` is referenced in the return type of `create_searcher_state` and in `searcher.lock()` calls inside `spawn_blocking` closures. `use std::sync::Arc` is already present. No import removals needed.

**`create_searcher_state`** return type changes from `Mutex<SessionSearcher>` to `Arc<Mutex<SessionSearcher>>`:

```rust
pub fn create_searcher_state(
    projects_dir: PathBuf,
    todos_dir: PathBuf,
    fs_provider: Arc<dyn FsProvider>,
) -> Arc<Mutex<SessionSearcher>> {
    Arc::new(Mutex::new(SessionSearcher::new(projects_dir, todos_dir, fs_provider)))
}
```

**`http_server.rs` update**: The current code at line 78 wraps the return value in `Arc::new(...)`:

```rust
// Before (would produce Arc<Arc<Mutex<...>>> after our change):
let searcher = Arc::new(create_searcher_state(projects_dir, todos_dir, Arc::new(LocalFsProvider::new())));

// After (create_searcher_state already returns Arc):
let searcher = create_searcher_state(projects_dir, todos_dir, Arc::new(LocalFsProvider::new()));
```

**`search_sessions`** command handler:

```rust
pub async fn search_sessions(
    project_id: String,
    query: String,
    max_results: Option<u32>,
    searcher: State<'_, Arc<Mutex<SessionSearcher>>>,
) -> Result<SearchSessionsResult, String> {
    let max = max_results.unwrap_or(50).min(100).max(1);
    if query.trim().is_empty() {
        return Ok(SearchSessionsResult {
            results: vec![],
            total_matches: 0,
            sessions_searched: 0,
            query: query.clone(),
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
```

**`search_all_projects`** command handler:

```rust
pub async fn search_all_projects(
    query: String,
    max_results: Option<u32>,
    searcher: State<'_, Arc<Mutex<SessionSearcher>>>,
) -> Result<SearchSessionsResult, String> {
    let max = max_results.unwrap_or(50).min(100).max(1);
    if query.trim().is_empty() {
        return Ok(SearchSessionsResult {
            results: vec![],
            total_matches: 0,
            sessions_searched: 0,
            query: query.clone(),
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
```

Note: `.min(100).max(1)` input validation is preserved from the current code. Empty query early-return is preserved before `spawn_blocking` to avoid wasting a blocking thread pool slot.

The `SessionSearcher` internal methods remain fully synchronous — no async conversion needed.

**`lib.rs`**: No change required. Tauri's `app.manage()` accepts any `Send + Sync` type; `Arc<Mutex<...>>` satisfies this.

**HTTP routes** (`http/routes/search.rs`):
- The cross-project search route is currently a stub (`// TODO`), out of scope.
- The single-project HTTP `search_sessions` route has the same C1 blocking issue (`state.searcher.lock()` in async context). This is **explicitly out of scope** for this fix — the HTTP server is a secondary access path and its search usage is lower volume. A `// TODO: Apply spawn_blocking (see docs/superpowers/specs/2026-03-30-search-mutex-memory-fix-design.md)` comment should be added at `http/routes/search.rs:39`.

### I2 Fix: bounded priority queue

**Files changed:**

- `src-tauri/src/types/domain.rs` — add `Ord` impl for `SearchResult`
- `src-tauri/src/discovery/session_searcher.rs` — rewrite `search_all_projects`

**`SearchResult` ordering** (by `timestamp` descending):

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

Safety note: All current fields (`String`, `u64`, `Option<String>`, `Option<u32>`) satisfy `Eq` semantics. If future fields include `f64`, the `Eq` impl would need revisiting since `f64::NAN != f64::NAN` violates `Eq`.

**`search_all_projects`** uses a bounded `BinaryHeap`:

```rust
use std::collections::BinaryHeap;
use std::cmp::Reverse;

pub fn search_all_projects(&mut self, query: &str, max_results: u32) -> SearchSessionsResult {
    let projects = self.project_scanner.scan();
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

Note: `is_partial` logic is preserved from the current code to maintain the API contract with the frontend.

**Edge case — `timestamp == 0`**: Results with `timestamp == 0` (fallback from `session_searcher.rs:299` `unwrap_or(0)`) all share the same priority. When the heap is full and `peek().timestamp == 0`, only results with `timestamp > 0` can evict. If all results have `timestamp == 0`, the heap retains the first `capacity` entries and stops replacing. This matches the current behavior (descending sort puts 0-timestamp results last) and is functionally correct.

**Known tradeoff — CPU/IO not reduced**: The `BinaryHeap` reduces memory from O(N × max) to O(max), but every project still undergoes full file I/O and string matching regardless of whether its results enter the top-N. Reducing CPU/IO would require a more complex approach (e.g., per-project pre-filtering by file timestamps) and is out of scope.

**Memory improvement:**

| Projects | Before (peak) | After (peak) |
|----------|---------------|--------------|
| 10       | ≤ 500         | ≤ 50         |
| 100      | ≤ 5,000       | ≤ 50         |
| 500      | ≤ 25,000      | ≤ 50         |

### What is NOT changed

- `search_sessions` (single-project search) internal logic — its memory is already bounded by `max_results` per call
- `SessionSearcher` cache — unbounded cache is a separate concern, out of scope
- HTTP cross-project search stub — out of scope
- HTTP single-project search C1 fix — lower volume, deferred to follow-up (TODO comment added)
- Frontend code — no API contract changes (`is_partial` preserved)

## Error handling

- `spawn_blocking` returns `JoinError` if the blocking task panics or is cancelled. We convert this to `String` via `.map_err(|e| e.to_string())?`, consistent with the existing error style.
- The `std::sync::Mutex::lock()` inside the blocking closure returns a `PoisonError` if a previous holder panicked. Same conversion applies.

## Testing

Existing tests in `session_searcher.rs` (lines 367-432) cover `search_sessions`. The I2 change modifies `search_all_projects` internals while preserving the public interface. Add the following tests:

1. **Multi-project top-N correctness**: Create 3+ projects with varying result counts and timestamps. Call `search_all_projects` and verify:
   - Returned results count ≤ `max_results`
   - Results are sorted by timestamp descending
   - The top results span multiple projects (not just the first batch)

2. **Heap eviction**: With `max_results = 2` and more than 2 matching results across projects, verify that the 2 results with the highest timestamps are returned (lower-timestamp results are evicted).

3. **Empty query**: Verify `search_all_projects` returns empty results for empty/whitespace-only queries without errors.

4. **C1 compile check**: Verify the `search_sessions` and `search_all_projects` Tauri command handlers compile with `State<'_, Arc<Mutex<SessionSearcher>>>` and the `spawn_blocking` wrapper. This is a compile-time verification, not a runtime test.
