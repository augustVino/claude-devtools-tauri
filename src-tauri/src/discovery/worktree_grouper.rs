//! WorktreeGrouper - Groups projects by git repository.
//!
//! Responsibilities:
//! - Group projects that belong to the same git repository
//! - Handle worktrees (main repo + worktrees grouped together)
//! - Sort worktrees by main first, then by most recent activity

use crate::parsing::git_identity::GitIdentityResolver;
use crate::types::domain::{Project, RepositoryGroup, RepositoryIdentity, Worktree};
use std::path::PathBuf;

/// WorktreeGrouper provides methods for grouping projects by git repository.
pub struct WorktreeGrouper {
    #[allow(dead_code)]
    projects_dir: PathBuf,
    git_resolver: GitIdentityResolver,
}

impl WorktreeGrouper {
    /// Create a new WorktreeGrouper.
    pub fn new(projects_dir: PathBuf) -> Self {
        Self {
            projects_dir,
            git_resolver: GitIdentityResolver::new(),
        }
    }

    /// Groups projects by git repository.
    /// Projects belonging to the same git repository (main repo + worktrees)
    /// are grouped together under a single RepositoryGroup.
    pub fn group_by_repository(&self, projects: Vec<Project>) -> Vec<RepositoryGroup> {
        if projects.is_empty() {
            return Vec::new();
        }

        // 1. Resolve repository identity for each project
        let mut project_identities: std::collections::HashMap<String, Option<RepositoryIdentity>> =
            std::collections::HashMap::new();
        let mut project_branches: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();

        for project in &projects {
            let identity = self.git_resolver.resolve_identity(&project.path);
            project_identities.insert(project.id.clone(), identity);

            let branch = self.git_resolver.get_branch(&project.path);
            project_branches.insert(project.id.clone(), branch);
        }

        // 2. Group projects by repository
        let mut repo_groups: std::collections::HashMap<
            String,
            RepoGroupData,
        > = std::collections::HashMap::new();

        for project in projects {
            let identity = project_identities.get(&project.id).and_then(|i| i.clone());
            let branch = project_branches.get(&project.id).cloned().flatten();

            // Use repository ID if available, otherwise use project ID
            let group_id = identity.as_ref().map(|i| i.id.clone()).unwrap_or_else(|| project.id.clone());

            let group = repo_groups.entry(group_id).or_insert_with(|| RepoGroupData {
                identity: identity.clone(),
                projects: Vec::new(),
                branches: std::collections::HashMap::new(),
            });

            group.projects.push(project.clone());
            if let Some(b) = branch {
                group.branches.insert(project.id, b);
            }
        }

        // 3. Convert to RepositoryGroup[]
        let mut repository_groups: Vec<RepositoryGroup> = Vec::new();

        for (group_id, group) in repo_groups {
            let mut worktrees: Vec<Worktree> = Vec::new();

            for project in &group.projects {
                let branch = group.branches.get(&project.id).cloned();
                let is_main_worktree = !self.git_resolver.is_worktree(&project.path);
                let source = self.git_resolver.detect_worktree_source(&project.path);
                let display_name = self.git_resolver.get_worktree_display_name(
                    &project.path,
                    &source,
                    branch.as_deref(),
                    is_main_worktree,
                );

                worktrees.push(Worktree {
                    id: project.id.clone(),
                    path: project.path.clone(),
                    name: display_name,
                    git_branch: branch,
                    is_main_worktree,
                    source,
                    sessions: project.sessions.clone(),
                    created_at: project.created_at,
                    most_recent_session: project.most_recent_session,
                });
            }

            // Filter out worktrees with 0 sessions
            let non_empty_worktrees: Vec<Worktree> = worktrees
                .into_iter()
                .filter(|wt| !wt.sessions.is_empty())
                .collect();

            // Skip this repository group if all worktrees are empty
            if non_empty_worktrees.is_empty() {
                continue;
            }

            // Sort worktrees: main first, then by most recent activity
            let mut sorted_worktrees = non_empty_worktrees;
            sorted_worktrees.sort_by(|a, b| {
                if a.is_main_worktree && !b.is_main_worktree {
                    std::cmp::Ordering::Less
                } else if !a.is_main_worktree && b.is_main_worktree {
                    std::cmp::Ordering::Greater
                } else {
                    b.most_recent_session.unwrap_or(0).cmp(&a.most_recent_session.unwrap_or(0))
                }
            });

            let total_sessions: u32 = sorted_worktrees.iter().map(|wt| wt.sessions.len() as u32).sum();
            let most_recent_session = sorted_worktrees
                .iter()
                .map(|wt| wt.most_recent_session.unwrap_or(0))
                .max()
                .unwrap_or(0);

            let identity = group.identity.clone();
            let name = group.identity.clone()
                .map(|i| i.name)
                .unwrap_or_else(|| {
                    group.projects.first().map(|p| p.name.clone()).unwrap_or_default()
                });

            repository_groups.push(RepositoryGroup {
                id: group_id,
                identity,
                worktrees: sorted_worktrees,
                name,
                most_recent_session: if most_recent_session > 0 { Some(most_recent_session) } else { None },
                total_sessions,
            });
        }

        // 4. Sort repository groups by most recent activity
        repository_groups.sort_by(|a, b| {
            b.most_recent_session.unwrap_or(0).cmp(&a.most_recent_session.unwrap_or(0))
        });

        repository_groups
    }

    /// Get worktree project ID (same as worktree ID).
    #[allow(dead_code)]
    pub fn get_worktree_project_id(&self, worktree_id: &str) -> String {
        worktree_id.to_string()
    }
}

/// Internal data structure for grouping projects.
struct RepoGroupData {
    identity: Option<RepositoryIdentity>,
    projects: Vec<Project>,
    branches: std::collections::HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group_by_repository_empty() {
        let grouper = WorktreeGrouper::new(PathBuf::from("/tmp/projects"));
        let groups = grouper.group_by_repository(Vec::new());
        assert!(groups.is_empty());
    }

    #[test]
    fn test_group_by_repository_single_project() {
        let grouper = WorktreeGrouper::new(PathBuf::from("/tmp/projects"));

        let project = Project {
            id: "test-project".to_string(),
            path: "/tmp/test-project".to_string(),
            name: "Test Project".to_string(),
            sessions: vec!["session-1".to_string()],
            created_at: 1000,
            most_recent_session: Some(2000),
        };

        let groups = grouper.group_by_repository(vec![project]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].worktrees.len(), 1);
    }

    #[test]
    fn test_get_worktree_project_id() {
        let grouper = WorktreeGrouper::new(PathBuf::from("/tmp/projects"));
        assert_eq!(grouper.get_worktree_project_id("worktree-123"), "worktree-123");
    }
}