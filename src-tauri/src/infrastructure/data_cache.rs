//! 数据缓存 — 基于 LRU 策略缓存已解析的会话数据，支持 TTL 过期自动淘汰。
//!
//! 从 Electron `DataCache.ts` 移植而来。使用 `moka` 库内置的
//! LRU 淘汰与 TTL 过期机制，无需手动管理定时器。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use moka::future::Cache;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// 类型定义
// ---------------------------------------------------------------------------

/// 缓存条目包装结构，携带模式版本号用于批量失效。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    value: serde_json::Value,
    version: u32,
}

/// 当前缓存模式版本号。当缓存结构发生变更时递增此值，
/// 以强制升级后重新解析。
const CURRENT_VERSION: u32 = 2;

// ---------------------------------------------------------------------------
// DataCache
// ---------------------------------------------------------------------------

/// 基于 `moka` 的 LRU 缓存，支持可配置容量与 TTL。
///
/// 当 `enabled` 为 `false` 时，所有读写操作为 no-op（与 Electron `DataCache.setEnabled()` 对齐）。
#[derive(Clone)]
pub struct DataCache {
    cache: Arc<Cache<String, CacheEntry>>,
    /// 使用 `Arc<AtomicBool>` 确保克隆后（如 watcher task）共享启用状态，
    /// 与 Electron 单实例引用语义对齐。
    enabled: Arc<AtomicBool>,
}

impl DataCache {
    /// 创建默认配置的缓存（50 条容量，10 分钟 TTL）。
    pub fn new() -> Self {
        Self::with_options(50, 10)
    }

    /// 创建指定容量与 TTL 的缓存。
    pub fn with_options(max_size: u64, ttl_minutes: u64) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_size)
            .time_to_live(Duration::from_secs(ttl_minutes * 60))
            .build();
        Self {
            cache: Arc::new(cache),
            enabled: Arc::new(AtomicBool::new(true)),
        }
    }

    /// 创建禁用状态的无操作缓存（与 Electron `new DataCache(50, 10, false)` 对齐）。
    pub fn disabled() -> Self {
        let cache = Cache::builder()
            .max_capacity(50)
            .time_to_live(Duration::from_secs(10 * 60))
            .build();
        Self {
            cache: Arc::new(cache),
            enabled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 设置缓存启用状态（与 Electron `setEnabled()` 对齐）。
    ///
    /// 由于 `enabled` 使用 `Arc<AtomicBool>`，此操作对所有克隆实例（如 watcher task）生效。
    pub async fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
        if !enabled {
            self.cache.invalidate_all();
            self.cache.run_pending_tasks().await;
        }
    }

    /// 返回缓存是否启用。
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    // ---- 会话缓存 ---------------------------------------------------------

    /// 获取已缓存的会话数据。未命中、过期或版本不匹配时返回 `None`。
    pub async fn get_session(
        &self,
        project_id: &str,
        session_id: &str,
    ) -> Option<serde_json::Value> {
        self.get(&Self::build_key(project_id, session_id)).await
    }

    /// 将会话数据存入缓存。
    pub async fn set_session(
        &self,
        project_id: &str,
        session_id: &str,
        value: serde_json::Value,
    ) {
        self.set(&Self::build_key(project_id, session_id), value).await;
    }

    // ---- 子代理缓存 -------------------------------------------------------

    /// 获取已缓存的子代理数据。未命中、过期或版本不匹配时返回 `None`。
    pub async fn get_subagent(
        &self,
        project_id: &str,
        session_id: &str,
        subagent_id: &str,
    ) -> Option<serde_json::Value> {
        self.get(&Self::build_subagent_key(project_id, session_id, subagent_id))
            .await
    }

    /// 将子代理数据存入缓存。
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

    // ---- 失效管理 ---------------------------------------------------------

    /// 使单条会话缓存条目失效。
    pub async fn invalidate_session(&self, project_id: &str, session_id: &str) {
        if !self.is_enabled() {
            return;
        }
        let key = Self::build_key(project_id, session_id);
        self.cache.invalidate(&key).await;

        // 同时移除该会话关联的所有子代理缓存条目
        self.invalidate_matching(&format!("-{session_id}-"), Some(project_id))
            .await;
    }

    /// 使指定项目下的所有缓存条目失效。
    pub async fn invalidate_project(&self, project_id: &str) {
        if !self.is_enabled() {
            return;
        }
        // 会话条目的键格式为 "{projectId}/"
        self.cache
            .run_pending_tasks()
            .await; // 确保迭代时能看到最新状态

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

    /// 清空整个缓存。
    pub async fn clear(&self) {
        self.cache.invalidate_all();
    }

    /// 返回缓存中当前条目数。
    pub async fn entry_count(&self) -> u64 {
        self.cache.entry_count()
    }

    // ---- 键构建辅助方法 ---------------------------------------------------

    fn build_key(project_id: &str, session_id: &str) -> String {
        format!("{project_id}/{session_id}")
    }

    fn build_subagent_key(project_id: &str, session_id: &str, subagent_id: &str) -> String {
        format!("subagent-{project_id}-{session_id}-{subagent_id}")
    }

    // ---- 内部实现 ---------------------------------------------------------

    async fn get(&self, key: &str) -> Option<serde_json::Value> {
        if !self.is_enabled() {
            return None;
        }
        let entry = self.cache.get(key).await?;
        if entry.version != CURRENT_VERSION {
            self.cache.invalidate(key).await;
            return None;
        }
        Some(entry.value)
    }

    async fn set(&self, key: &str, value: serde_json::Value) {
        if !self.is_enabled() {
            return;
        }
        let entry = CacheEntry {
            value,
            version: CURRENT_VERSION,
        };
        self.cache.insert(key.to_string(), entry).await;
    }

    /// 移除键中包含指定标记的条目，可额外匹配项目前缀（用于子代理键）。
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

    /// 判断 `key` 是否属于指定项目（会话或子代理）。
    fn key_belongs_to_project(key: &str, project_id: &str) -> bool {
        // 会话键格式: "{projectId}/{sessionId}"
        if let Some(rest) = key.strip_prefix(project_id) {
            if rest.starts_with('/') || rest.starts_with("::") {
                return true;
            }
        }
        // 子代理键格式: "subagent-{projectId}-..."
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
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn json_val(n: u32) -> serde_json::Value {
        serde_json::json!({ "n": n })
    }

    // -- 设置与读取 --------------------------------------------------------

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

    // -- TTL 过期 -----------------------------------------------------------

    #[tokio::test]
    async fn ttl_expiry_returns_none() {
        // TTL 设为 0 分钟，使条目立即过期
        let cache = DataCache::with_options(50, 0);
        let val = json_val(99);

        cache.set_session("proj-a", "sess-ttl", val).await;

        // 短暂等待，让 moka 的异步过期机制执行
        tokio::time::sleep(Duration::from_millis(5)).await;

        let got = cache.get_session("proj-a", "sess-ttl").await;
        assert!(got.is_none());
    }

    // -- 最大容量淘汰 -------------------------------------------------------

    #[tokio::test]
    async fn max_size_eviction() {
        let cache = DataCache::with_options(3, 10);

        // 向容量为 3 的缓存中插入 4 条数据
        for i in 0..4 {
            cache
                .set_session("proj", &format!("sess-{i}"), json_val(i))
                .await;
        }

        // 至少应有一条较早的条目被淘汰。
        // 最先插入的 (sess-0) 是 LRU 淘汰候选。
        let _first = cache.get_session("proj", "sess-0").await;
        // 由于 moka 内部批处理机制，无法确定具体哪条被淘汰，
        // 但可以验证缓存数量未超过最大容量。
        let count = cache.entry_count().await;
        assert!(count <= 3, "cache should not exceed max_capacity, got {count}");

        // 最近写入的条目必须仍在缓存中。
        let last = cache.get_session("proj", "sess-3").await;
        assert!(last.is_some(), "most-recent entry should survive eviction");
    }

    // -- 失效操作 -----------------------------------------------------------

    #[tokio::test]
    async fn invalidate_session() {
        let cache = DataCache::new();

        cache
            .set_session("proj-a", "sess-inv", json_val(1))
            .await;
        cache
            .set_subagent("proj-a", "sess-inv", "sub-1", json_val(2))
            .await;
        // 无关联条目
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

    // -- 禁用状态测试（与 Electron DataCache.setEnabled(false) 对齐）-----------

    #[tokio::test]
    async fn disabled_cache_set_is_noop() {
        let cache = DataCache::disabled();
        assert!(!cache.is_enabled());

        cache.set_session("proj", "sess", json_val(1)).await;
        let got = cache.get_session("proj", "sess").await;
        assert!(got.is_none(), "disabled cache should return None on get");
    }

    #[tokio::test]
    async fn set_enabled_toggles_cache() {
        let cache = DataCache::new();
        assert!(cache.is_enabled());

        // 写入一条数据
        cache.set_session("proj", "sess", json_val(42)).await;
        assert!(cache.get_session("proj", "sess").await.is_some());

        // 禁用后应清空且 get 返回 None
        cache.set_enabled(false).await;
        assert!(!cache.is_enabled());
        assert!(cache.get_session("proj", "sess").await.is_none());

        // 重新启用后可以正常使用
        cache.set_enabled(true).await;
        assert!(cache.is_enabled());
        cache.set_session("proj", "sess2", json_val(99)).await;
        assert!(cache.get_session("proj", "sess2").await.is_some());
    }

    #[tokio::test]
    async fn enabled_state_shared_across_clones() {
        let cache = DataCache::new();
        let clone = cache.clone();

        // 两者初始都启用
        assert!(cache.is_enabled());
        assert!(clone.is_enabled());

        // 在原始实例上禁用，克隆也应反映
        cache.set_enabled(false).await;
        assert!(!cache.is_enabled());
        assert!(!clone.is_enabled(), "clone should see enabled=false after set_enabled on original");

        // 克隆上重新启用，原始也应反映
        clone.set_enabled(true).await;
        assert!(cache.is_enabled(), "original should see enabled=true after set_enabled on clone");
    }
}
