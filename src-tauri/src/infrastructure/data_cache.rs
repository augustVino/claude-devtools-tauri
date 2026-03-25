//! LRU cache for parsed session data with TTL-based expiration.
//!
//! Port of the Electron `DataCache.ts` service. Uses `moka` for built-in
//! LRU eviction and TTL expiry so we do not manage timers manually.

use std::sync::Arc;
use std::time::Duration;

use moka::future::Cache;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Wrapped cache payload carrying a schema version for bulk invalidation.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    value: serde_json::Value,
    version: u32,
}

/// Current cache schema version. Bump when the cached structure changes
/// to force re-parsing after an upgrade.
const CURRENT_VERSION: u32 = 2;

// ---------------------------------------------------------------------------
// DataCache
// ---------------------------------------------------------------------------

/// LRU cache backed by `moka` with configurable capacity and TTL.
pub struct DataCache {
    cache: Arc<Cache<String, CacheEntry>>,
}

impl DataCache {
    /// Creates a new cache with default settings (50 entries, 10 min TTL).
    pub fn new() -> Self {
        Self::with_options(50, 10)
    }

    /// Creates a cache with custom capacity and TTL.
    pub fn with_options(max_size: u64, ttl_minutes: u64) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_size)
            .time_to_live(Duration::from_secs(ttl_minutes * 60))
            .build();
        Self {
            cache: Arc::new(cache),
        }
    }

    // ---- Session cache ----------------------------------------------------

    /// Retrieves a cached session, returning `None` on miss, expiry, or
    /// version mismatch.
    pub async fn get_session(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Option<serde_json::Value> {
        self.get(&Self::build_key(project_id, session_id)).await
    }

    /// Stores a session in the cache.
    pub async fn set_session(
        &self,
        project_id: &str,
        session_id: &str,
        value: serde_json::Value,
    ) {
        self.set(&Self::build_key(project_id, session_id), value).await;
    }

    // ---- Subagent cache --------------------------------------------------

    /// Retrieves a cached subagent, returning `None` on miss, expiry, or
    /// version mismatch.
    pub async fn get_subagent(
        &self,
        project_id: &str,
        session_id: &str,
        subagent_id: &str,
    ) -> Option<serde_json::Value> {
        self.get(&Self::build_subagent_key(project_id, session_id, subagent_id))
            .await
    }

    /// Stores a subagent in the cache.
    pub async fn set_subagent(
        &self,
        project_id: &str,
        session_id: &str,
        subagent_id: &str,
        value: serde_json::Value,
    ) {
        self.set(
            &Self::build_subagent_key(project_id, session_id, subagent_id),
            value,
        )
        .await;
    }

    // ---- Invalidation ----------------------------------------------------

    /// Invalidates a single session entry.
    pub async fn invalidate_session(&self, project_id: &str, session_id: &str) {
        let key = Self::build_key(project_id, session_id);
        self.cache.invalidate(&key).await;

        // Also remove any subagent entries for this session.
        self.invalidate_matching(&format!("-{session_id}-"), Some(project_id))
            .await;
    }

    /// Invalidates all entries belonging to a project.
    pub async fn invalidate_project(&self, project_id: &str) {
        // Session entries: key starts with "{projectId}/"
        self.cache
            .run_pending_tasks()
            .await; // ensure iteration sees latest state

        let keys: Vec<String> = self
            .cache
            .iter()
            .filter_map(|(k, _)| {
                if Self::key_belongs_to_project(&k, project_id) {
                    Some(k.to_string())
                } else {
                    None
                }
            })
            .collect();

        for key in keys {
            self.cache.invalidate(&key).await;
        }
    }

    /// Clears the entire cache.
    pub async fn clear(&self) {
        self.cache.invalidate_all();
    }

    /// Returns the number of entries currently in the cache.
    pub async fn entry_count(&self) -> u64 {
        self.cache.entry_count()
    }

    // ---- Key helpers -----------------------------------------------------

    fn build_key(project_id: &str, session_id: &str) -> String {
        format!("{project_id}/{session_id}")
    }

    fn build_subagent_key(project_id: &str, session_id: &str, subagent_id: &str) -> String {
        format!("subagent-{project_id}-{session_id}-{subagent_id}")
    }

    // ---- Internals -------------------------------------------------------

    async fn get(&self, key: &str) -> Option<serde_json::Value> {
        let entry = self.cache.get(key).await?;
        if entry.version != CURRENT_VERSION {
            self.cache.invalidate(key).await;
            return None;
        }
        Some(entry.value)
    }

    async fn set(&self, key: &str, value: serde_json::Value) {
        let entry = CacheEntry {
            value,
            version: CURRENT_VERSION,
        };
        self.cache.insert(key.to_string(), entry).await;
    }

    /// Removes entries whose key contains `token` and optionally matches a
    /// project prefix (for subagent keys).
    async fn invalidate_matching(&self, token: &str, project_id: Option<&str>) {
        let keys: Vec<String> = self
            .cache
            .iter()
            .filter_map(|(k, _)| {
                if !k.contains(token) {
                    return None;
                }
                match project_id {
                    Some(pid) if k.starts_with("subagent-") => {
                        let prefix = format!("subagent-{pid}");
                        if k.starts_with(&prefix) || k.starts_with(&format!("{prefix}::")) {
                            Some(k.to_string())
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            })
            .collect();

        for key in keys {
            self.cache.invalidate(&key).await;
        }
    }

    /// Returns `true` when `key` belongs to `project_id` (session or subagent).
    fn key_belongs_to_project(key: &str, project_id: &str) -> bool {
        // Session key: "{projectId}/{sessionId}"
        if let Some(rest) = key.strip_prefix(project_id) {
            if rest.starts_with('/') || rest.starts_with("::") {
                return true;
            }
        }
        // Subagent key: "subagent-{projectId}-..."
        if let Some(rest) = key.strip_prefix("subagent-") {
            if let Some(after) = rest.strip_prefix(project_id) {
                if after.starts_with('-') || after.starts_with("::") {
                    return true;
                }
            }
        }
        false
    }
}

impl Default for DataCache {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn json_val(n: u32) -> serde_json::Value {
        serde_json::json!({ "n": n })
    }

    // -- set & get ---------------------------------------------------------

    #[tokio::test]
    async fn set_and_get_session() {
        let cache = DataCache::new();
        let val = json_val(1);

        cache.set_session("proj-a", "sess-1", val.clone()).await;
        let got = cache.get_session("proj-a", "sess-1").await;

        assert_eq!(got, Some(val));
    }

    #[tokio::test]
    async fn get_miss_returns_none() {
        let cache = DataCache::new();
        let got = cache.get_session("proj-x", "sess-y").await;
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn set_and_get_subagent() {
        let cache = DataCache::new();
        let val = json_val(42);

        cache
            .set_subagent("proj-a", "sess-1", "sub-3", val.clone())
            .await;
        let got = cache
            .get_subagent("proj-a", "sess-1", "sub-3")
            .await;

        assert_eq!(got, Some(val));
    }

    // -- TTL expiry --------------------------------------------------------

    #[tokio::test]
    async fn ttl_expiry_returns_none() {
        // 1 ms TTL so entries expire immediately
        let cache = DataCache::with_options(50, 0);
        let val = json_val(99);

        cache.set_session("proj-a", "sess-ttl", val).await;

        // Brief sleep to let moka's async expiry run
        tokio::time::sleep(Duration::from_millis(5)).await;

        let got = cache.get_session("proj-a", "sess-ttl").await;
        assert!(got.is_none());
    }

    // -- max size eviction -------------------------------------------------

    #[tokio::test]
    async fn max_size_eviction() {
        let cache = DataCache::with_options(3, 10);

        // Insert 4 entries into a capacity-3 cache
        for i in 0..4 {
            cache
                .set_session("proj", &format!("sess-{i}"), json_val(i))
                .await;
        }

        // At least one of the earlier entries should have been evicted.
        // The first inserted (sess-0) is the LRU candidate.
        let _first = cache.get_session("proj", "sess-0").await;
        // We cannot guarantee which was evicted due to moka's internal
        // batching, but we *can* verify the cache did not grow beyond
        // capacity.
        let count = cache.entry_count().await;
        assert!(count <= 3, "cache should not exceed max_capacity, got {count}");

        // The most recent entry must still be present.
        let last = cache.get_session("proj", "sess-3").await;
        assert!(last.is_some(), "most-recent entry should survive eviction");
    }

    // -- invalidation ------------------------------------------------------

    #[tokio::test]
    async fn invalidate_session() {
        let cache = DataCache::new();

        cache
            .set_session("proj-a", "sess-inv", json_val(1))
            .await;
        cache
            .set_subagent("proj-a", "sess-inv", "sub-1", json_val(2))
            .await;
        // unrelated entry
        cache
            .set_session("proj-b", "sess-other", json_val(3))
            .await;

        cache.invalidate_session("proj-a", "sess-inv").await;

        assert!(cache.get_session("proj-a", "sess-inv").await.is_none());
        assert!(
            cache
                .get_subagent("proj-a", "sess-inv", "sub-1")
                .await
                .is_none()
        );
        assert!(
            cache
                .get_session("proj-b", "sess-other")
                .await
                .is_some()
        );
    }

    #[tokio::test]
    async fn invalidate_project() {
        let cache = DataCache::new();

        cache
            .set_session("proj-x", "s1", json_val(10))
            .await;
        cache
            .set_session("proj-x", "s2", json_val(11))
            .await;
        cache
            .set_subagent("proj-x", "s1", "sub-a", json_val(12))
            .await;
        // unrelated
        cache
            .set_session("proj-y", "s1", json_val(20))
            .await;

        cache.invalidate_project("proj-x").await;

        assert!(cache.get_session("proj-x", "s1").await.is_none());
        assert!(cache.get_session("proj-x", "s2").await.is_none());
        assert!(
            cache
                .get_subagent("proj-x", "s1", "sub-a")
                .await
                .is_none()
        );
        assert!(cache.get_session("proj-y", "s1").await.is_some());
    }

    #[tokio::test]
    async fn clear_empties_cache() {
        let cache = DataCache::new();
        cache.set_session("p", "s", json_val(1)).await;
        cache.clear().await;
        assert_eq!(cache.entry_count().await, 0);
    }
}
