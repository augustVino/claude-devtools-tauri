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
- `src-tauri/src/http/state.rs` (already uses `Arc<Mutex<...>>`, no change needed)

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

**`search_sessions` and `search_all_projects`** command handlers wrap the synchronous call in `spawn_blocking`:

```rust
pub async fn search_all_projects(
    query: String,
    max_results: Option<u32>,
    searcher: State<'_, Arc<Mutex<SessionSearcher>>>,
) -> Result<SearchSessionsResult, String> {
    let max = max_results.unwrap_or(50);
    let searcher = searcher.inner().clone();
    tokio::task::spawn_blocking(move || {
        let mut s = searcher.lock().map_err(|e| e.to_string())?;
        Ok(s.search_all_projects(&query, max))
    })
    .await
    .map_err(|e| e.to_string())?
}
```

The same pattern applies to `search_sessions`. The `SessionSearcher` internal methods remain fully synchronous — no async conversion needed.

**`lib.rs`**: No change required. Tauri's `app.manage()` accepts any `Send + Sync` type; `Arc<Mutex<...>>` satisfies this.

**HTTP routes** (`http/routes/search.rs`): The cross-project search route is currently a stub (`// TODO`). When implemented, it can use the same `spawn_blocking` pattern with the existing `Arc<Mutex<SessionSearcher>>` in `HttpState`.

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
        is_partial: None,
    }
}
```

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
- Frontend code — no API contract changes

## Error handling

- `spawn_blocking` returns `JoinError` if the blocking task panics or is cancelled. We convert this to `String` via `.map_err(|e| e.to_string())?`, consistent with the existing error style.
- The `std::sync::Mutex::lock()` inside the blocking closure returns a `PoisonError` if a previous holder panicked. Same conversion applies.
