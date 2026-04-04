//! 仓库范围检查 —— REPOSITORY_ID_CACHE、RepositoryScopeTarget、matches_repository_scope()、pre_resolve_repository_ids()。

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::RwLock;

use crate::parsing::git_identity::GitIdentityResolver;
use crate::utils::path_decoder;

/// 线程安全的 projectId -> repositoryId 映射缓存。
static REPOSITORY_ID_CACHE: LazyLock<RwLock<HashMap<String, Option<String>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// 预解析仓库 ID 的目标。
#[derive(Debug, Clone)]
pub struct RepositoryScopeTarget {
    pub project_id: String,
    pub cwd_hint: Option<String>,
}

/// 检查项目是否匹配触发器的仓库范围。
///
/// 若未指定仓库 ID，则触发器适用于所有仓库。
/// 若指定了仓库 ID 但无法从缓存中解析项目的仓库 ID，
/// 则触发器不匹配。
///
/// # 参数
/// * `project_id` - 编码后的项目 ID（例如 "-Users-username-myproject"）
/// * `repository_ids` - 可选的仓库组 ID 列表，用于限定触发器作用范围
///
/// # 返回值
/// 若触发器应应用于此项目则返回 `true`，否则返回 `false`。
pub fn matches_repository_scope(
    project_id: &str,
    repository_ids: Option<&[String]>,
) -> bool {
    // 未指定仓库 ID 时，触发器适用于所有仓库
    let ids = match repository_ids {
        Some(ids) if !ids.is_empty() => ids,
        _ => return true,
    };

    // 从缓存中获取此项目的仓库 ID
    let repo_id = {
        let cache = REPOSITORY_ID_CACHE
            .read()
            .expect("repository_id_cache lock poisoned");
        cache.get(project_id).cloned()
    };

    let resolved_id = match repo_id {
        Some(Some(id)) => id,
        _ => return false,
    };

    // 检查仓库 ID 是否匹配任意已配置的 ID
    ids.iter().any(|id| id == &resolved_id)
}

/// 预解析多个项目的仓库 ID。
///
/// 在检查触发器之前调用此函数以填充缓存。
/// 使用 `GitIdentityResolver` 进行实际解析。
///
/// # 参数
/// * `targets` - 待解析的项目 ID 列表（或带 cwd 提示的目标）
pub fn pre_resolve_repository_ids(targets: &[RepositoryScopeTarget]) {
    let resolver = GitIdentityResolver;

    // Deduplicate by project_id, preferring targets with cwd_hint
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut unique: Vec<(usize, &RepositoryScopeTarget)> = Vec::new();

    for (i, target) in targets.iter().enumerate() {
        match seen.get(&target.project_id) {
            Some(&prev_idx) => {
                // Replace with cwd_hint version if current has hint and previous doesn't
                let (_, prev_target) = unique[prev_idx];
                if target.cwd_hint.is_some() && prev_target.cwd_hint.is_none() {
                    unique[prev_idx] = (i, target);
                }
            }
            None => {
                seen.insert(target.project_id.clone(), unique.len());
                unique.push((i, target));
            }
        }
    }

    for (_i, target) in unique {
        let project_path = path_decoder::decode_path(&target.project_id);
        let resolved_path = target
            .cwd_hint
            .as_deref()
            .unwrap_or(&project_path);

        let identity = resolver.resolve_identity(resolved_path);
        let repo_id = identity.map(|id| id.id);

        let mut cache = REPOSITORY_ID_CACHE
            .write()
            .expect("repository_id_cache lock poisoned");
        cache.insert(target.project_id.clone(), repo_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_repository_scope_no_ids() {
        // No repository IDs -> always matches
        assert!(matches_repository_scope("-Users-test-project", None));
        assert!(matches_repository_scope("-Users-test-project", Some(&[])));
    }

    #[test]
    fn test_matches_repository_scope_cached_match() {
        let mut cache = REPOSITORY_ID_CACHE
            .write()
            .expect("repository_id_cache lock poisoned");
        cache.insert(
            "-Users-test-project".to_string(),
            Some("repo-abc-123".to_string()),
        );
        drop(cache);

        let result = matches_repository_scope(
            "-Users-test-project",
            Some(&["repo-abc-123".to_string()]),
        );
        assert!(result);
    }

    #[test]
    fn test_matches_repository_scope_cached_no_match() {
        let mut cache = REPOSITORY_ID_CACHE
            .write()
            .expect("repository_id_cache lock poisoned");
        cache.insert(
            "-Users-test-project".to_string(),
            Some("repo-xyz-789".to_string()),
        );
        drop(cache);

        let result = matches_repository_scope(
            "-Users-test-project",
            Some(&["repo-abc-123".to_string()]),
        );
        assert!(!result);
    }

    #[test]
    fn test_matches_repository_scope_not_cached() {
        // Not in cache -> should return false
        let result = matches_repository_scope(
            "-Users-unknown-project",
            Some(&["repo-abc-123".to_string()]),
        );
        assert!(!result);
    }

    #[test]
    fn test_matches_repository_scope_cached_none() {
        // Cached as None (unresolvable) -> should return false
        let mut cache = REPOSITORY_ID_CACHE
            .write()
            .expect("repository_id_cache lock poisoned");
        cache.insert("-Users-test-project".to_string(), None);
        drop(cache);

        let result = matches_repository_scope(
            "-Users-test-project",
            Some(&["repo-abc-123".to_string()]),
        );
        assert!(!result);
    }

    #[test]
    fn test_pre_resolve_repository_ids_noop() {
        // Just verify it doesn't panic
        let targets = vec![RepositoryScopeTarget {
            project_id: "-Users-test".to_string(),
            cwd_hint: None,
        }];
        pre_resolve_repository_ids(&targets);
    }
}
