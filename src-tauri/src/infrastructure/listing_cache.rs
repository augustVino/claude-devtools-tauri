//! Claude 侧 listing 结果缓存（项目列表 / 会话列表）。
//!
//! # 为什么
//!
//! `ProjectScanner::scan_async` / `list_sessions_async` 每次调用都全量重扫：
//! 每会话一次 200 行预览头读 + 一次 todo 读（SFTP 上各 ~百 ms 往返，batch=8
//! 并发也只是线性折减）。侧栏的 file-change 刷新风暴 / 分页（每页都先全量
//! 再切片）/ 项目切换让同一项目在一分钟内被全量扫描十余次 —— SSH 高延迟
//! 链路上这就是「选项目秒开变数秒」的元凶（2026-08 实测）。
//!
//! # 语义
//!
//! - 只缓存 **claude 纯结果**（scanner 输出）；多 agent 的 merge/append 每次
//!   内存合成（extra 部分由 `agents::AggCache` 挡 IO），两层缓存互不嵌套、
//!   失效互不牵连；
//! - TTL 60s 兜底；**实时性由 watcher 事件失效保证**（file-change 按 pid 清
//!   sessions + projects，todo-change 全清 sessions —— todo 文件名不含 pid）；
//! - key 含 projects_dir，local/ssh 上下文天然隔离。

use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::types::domain::{Project, Session};

const LISTING_TTL: Duration = Duration::from_secs(60);

struct ListingCache {
    /// projects_dir → (生成时刻, 项目列表)
    projects: std::collections::HashMap<String, (Instant, Vec<Project>)>,
    /// `{projects_dir}::{project_id}` → (生成时刻, 会话列表)
    sessions: std::collections::HashMap<String, (Instant, Vec<Session>)>,
}

fn cache() -> &'static Mutex<ListingCache> {
    static CACHE: OnceLock<Mutex<ListingCache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(ListingCache {
            projects: std::collections::HashMap::new(),
            sessions: std::collections::HashMap::new(),
        })
    })
}

fn projects_key(projects_dir: &Path) -> String {
    projects_dir.to_string_lossy().to_string()
}

fn sessions_key(projects_dir: &Path, project_id: &str) -> String {
    format!("{}::{}", projects_dir.to_string_lossy(), project_id)
}

/// TTL 内的缓存项目列表。
pub fn get_projects(projects_dir: &Path) -> Option<Vec<Project>> {
    let c = cache().lock().unwrap();
    c.projects
        .get(&projects_key(projects_dir))
        .filter(|(at, _)| at.elapsed() < LISTING_TTL)
        .map(|(_, v)| v.clone())
}

/// 写入项目列表缓存。
pub fn set_projects(projects_dir: &Path, projects: &[Project]) {
    cache().lock().unwrap().projects.insert(
        projects_key(projects_dir),
        (Instant::now(), projects.to_vec()),
    );
}

/// TTL 内的缓存会话列表。
pub fn get_sessions(projects_dir: &Path, project_id: &str) -> Option<Vec<Session>> {
    let c = cache().lock().unwrap();
    c.sessions
        .get(&sessions_key(projects_dir, project_id))
        .filter(|(at, _)| at.elapsed() < LISTING_TTL)
        .map(|(_, v)| v.clone())
}

/// 写入会话列表缓存。
pub fn set_sessions(projects_dir: &Path, project_id: &str, sessions: &[Session]) {
    cache().lock().unwrap().sessions.insert(
        sessions_key(projects_dir, project_id),
        (Instant::now(), sessions.to_vec()),
    );
}

/// file-change 事件失效：清该项目的 sessions（含 worktree 复合 id 变体 ——
/// 按 base 前缀匹配）与 projects（项目行携带 session 计数/最近活跃）。
pub fn invalidate_project(project_id: &str) {
    let base = crate::utils::extract_base_dir(project_id);
    let mut c = cache().lock().unwrap();
    c.sessions.retain(|k, _| {
        // key 尾段（首个 `::` 之后）不以该 base 开头才保留。注意 project_id
        // 自身可含 `::`（worktree 复合 id `-p::wt1`）—— 必须取**首个**分隔符
        //（projects_dir 路径不含 `::`），rsplit 会把复合 id 劈成两半导致
        // worktree 变体永远命中不了前缀（测试抓到过）
        match k.split_once("::") {
            Some((_, pid)) => !pid.starts_with(base),
            None => true,
        }
    });
    c.projects.clear();
}

/// todo-change 事件失效：todo 文件名只有 session id，无法定位项目 →
/// sessions 全清（重建成本可接受，频率低）。
pub fn invalidate_all_sessions() {
    cache().lock().unwrap().sessions.clear();
}



#[cfg(test)]
mod tests {
    use super::*;

    fn proj_dir(tag: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(format!("/tmp/lc-test-{tag}"))
    }

    /// TTL 命中 / 失效 / 事件失效的最小语义闭环。
    #[test]
    fn get_set_and_invalidation_semantics() {
        // 等效全清（invalidate_all 无生产调用方已删；key 含 projects_dir
        // 天然上下文隔离，不需要切换清理）
        {
            let mut c = cache().lock().unwrap();
            c.projects.clear();
            c.sessions.clear();
        }
        let pd = proj_dir("semantics");

        assert!(get_projects(&pd).is_none());
        set_projects(&pd, &[Project {
            id: "-p".into(),
            path: "/p".into(),
            name: "p".into(),
            sessions: vec![],
            created_at: 0,
            most_recent_session: None,
        }]);
        assert!(get_projects(&pd).is_some(), "fresh entry must hit");

        assert!(get_sessions(&pd, "-p").is_none());
        set_sessions(&pd, "-p", &[]);
        assert!(get_sessions(&pd, "-p").is_some());

        // 项目级失效：sessions 按 base 前缀清（含 worktree 变体），projects 全清
        set_sessions(&pd, "-p::wt1", &[]);
        invalidate_project("-p");
        assert!(get_sessions(&pd, "-p").is_none());
        assert!(get_sessions(&pd, "-p::wt1").is_none(), "worktree variant must be evicted");
        assert!(get_projects(&pd).is_none());

        // 其他项目不受牵连
        set_sessions(&pd, "-other", &[]);
        invalidate_project("-p");
        assert!(get_sessions(&pd, "-other").is_some());

        // todo 失效：sessions 全清
        invalidate_all_sessions();
        assert!(get_sessions(&pd, "-other").is_none());

        // 不同 projects_dir（上下文）隔离
        let pd2 = proj_dir("other-ctx");
        set_sessions(&pd, "-x", &[]);
        invalidate_project("-x"); // 清 pd 的
        set_sessions(&pd2, "-x", &[]);
        assert!(get_sessions(&pd2, "-x").is_some());
    }
}
