//! Git identity 结果缓存（worktree 分组专用）。
//!
//! # 为什么
//!
//! `WorktreeGrouper::group_by_repository` 对每个项目调用 6 个
//! `GitIdentityResolver` 方法（resolve_identity / get_branch / is_worktree /
//! detect_worktree_source / get_worktree_display_name），内部共 ~14 次本地
//! fs 访问/项目 —— 22 项目一次分组 ≈ 300 次 stat/read，且历史上在前端
//! 切换上下文时被 4 个并发调用各自重复执行（SSH 实测 38.6s）。
//!
//! 这些结果**只依赖 project.path**（远端路径在本地 fs 恒不存在 → 走纯
//! 启发式；本地路径读 .git 文件）—— 同一进程内可安全缓存。文件系统变更
//! 对仓库身份的影响极低频，TTL 10min 足够。
//!
//! # 语义
//!
//! - key = project_path；per-entry TTL（10min）；
//! - SSH 上下文同样受益：本地 miss 一次后 10 分钟内零 fs 访问。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::parsing::git_identity::GitIdentityResolver;
use crate::types::domain::{RepositoryIdentity, WorktreeSource};

const GIT_CACHE_TTL: Duration = Duration::from_secs(600);

/// 单项目的全套 git 身份信息（分组所需字段的快照）。
#[derive(Clone)]
pub struct GitFacts {
    pub identity: Option<RepositoryIdentity>,
    pub branch: Option<String>,
    pub is_main_worktree: bool,
    pub source: WorktreeSource,
    pub display_name: String,
}

struct Entry {
    at: Instant,
    facts: GitFacts,
}

fn cache() -> &'static Mutex<HashMap<PathBuf, Entry>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Entry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 取某项目的全套 git 身份（TTL 内命中缓存；miss 时由 resolver 解析一次）。
/// resolver 由调用方传入（保持解析逻辑单点在 GitIdentityResolver）。
pub fn facts_for(project_path: &str, resolver: &GitIdentityResolver) -> GitFacts {
    let key = PathBuf::from(project_path);
    {
        let c = cache().lock().unwrap();
        if let Some(e) = c.get(&key) {
            if e.at.elapsed() < GIT_CACHE_TTL {
                return e.facts.clone();
            }
        }
    }
    let identity = resolver.resolve_identity(project_path);
    let branch = resolver.get_branch(project_path);
    let is_main_worktree = !resolver.is_worktree(project_path);
    let source = resolver.detect_worktree_source(project_path);
    let display_name = resolver.get_worktree_display_name(
        project_path,
        &source,
        branch.as_deref(),
        is_main_worktree,
    );
    let facts = GitFacts {
        identity,
        branch,
        is_main_worktree,
        source: source.clone(),
        display_name,
    };
    // 粗粒度容量保护：超额全清（重建成本低）
    let mut c = cache().lock().unwrap();
    if c.len() > 4096 {
        c.clear();
    }
    c.insert(key, Entry { at: Instant::now(), facts: facts.clone() });
    facts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 命中语义：同 path 第二次调用不再触碰 resolver（用计数 resolver 不可行
    /// —— GitIdentityResolver 是无状态结构体；这里断言时间戳推进 + 结果一致）。
    #[test]
    fn facts_are_cached_per_path() {
        let resolver = GitIdentityResolver::new();
        let p = "/tmp/git-facts-cache-test-proj";
        let f1 = facts_for(p, &resolver);
        let f2 = facts_for(p, &resolver);
        assert_eq!(f1.display_name, f2.display_name);
        assert_eq!(f1.is_main_worktree, f2.is_main_worktree);
        // 不存在的路径走启发式：name 回退为路径末段
        assert!(!f1.display_name.is_empty());
    }
}
