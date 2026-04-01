//! Subproject Registry for cwd-based project splitting.
//!
//! When a single encoded project directory contains sessions from multiple
//! working directories (cwd), the project is split into subprojects identified
//! by composite IDs: `{baseDir}::{sha256(cwd)[0..8]}`.

use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

struct SubprojectEntry {
    base_dir: String,
    cwd: String,
    session_ids: HashSet<String>,
}

pub struct SubprojectRegistry {
    entries: HashMap<String, SubprojectEntry>,
}

impl SubprojectRegistry {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Register a subproject and return its composite ID.
    pub fn register(
        &mut self,
        base_dir: &str,
        cwd: &str,
        session_ids: Vec<String>,
    ) -> String {
        let hash = format!("{:x}", Sha256::digest(cwd.as_bytes()))[..8].to_string();
        let composite_id = format!("{base_dir}::{hash}");
        self.entries.insert(
            composite_id.clone(),
            SubprojectEntry {
                base_dir: base_dir.to_string(),
                cwd: cwd.to_string(),
                session_ids: session_ids.into_iter().collect(),
            },
        );
        composite_id
    }

    /// Check if a project ID is a composite (sub-split) ID.
    pub fn is_composite(&self, project_id: &str) -> bool {
        project_id.contains("::")
    }

    /// Get the set of session IDs for a composite project ID.
    pub fn get_session_filter(&self, project_id: &str) -> Option<&HashSet<String>> {
        self.entries.get(project_id).map(|e| &e.session_ids)
    }

    /// Get the cwd for a composite project ID.
    pub fn get_cwd(&self, project_id: &str) -> Option<&str> {
        self.entries.get(project_id).map(|e| e.cwd.as_str())
    }

    /// Clear all entries (called on each full re-scan).
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}
