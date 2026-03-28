use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};
use tools::{Result as ToolResult, ToolError, ToolWriteGuard};
use types::AgentId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteLeaseConflict {
    pub requested: String,
    pub owner: AgentId,
    pub owner_path: String,
}

#[derive(Clone, Default)]
pub struct WriteLeaseManager {
    leases: Arc<RwLock<LeaseIndex>>,
}

impl WriteLeaseManager {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn claim(&self, agent_id: &AgentId, paths: &[PathBuf]) -> Result<(), WriteLeaseConflict> {
        let requested = paths
            .iter()
            .map(|path| normalize_path(path))
            .collect::<Vec<_>>();
        let mut guard = self.leases.write().expect("write lease lock");
        for path in &requested {
            if let Some((owned_path, owner)) = guard.find_conflict(agent_id, path) {
                return Err(WriteLeaseConflict {
                    requested: path.display().to_string(),
                    owner,
                    owner_path: owned_path.display().to_string(),
                });
            }
        }
        for path in requested {
            guard.insert(agent_id, path);
        }
        Ok(())
    }

    pub fn release(&self, agent_id: &AgentId) {
        self.leases
            .write()
            .expect("write lease lock")
            .release(agent_id);
    }

    #[must_use]
    pub fn claimed_paths(&self, agent_id: &AgentId) -> Vec<String> {
        self.leases
            .read()
            .expect("write lease lock")
            .claimed_paths(agent_id)
            .into_iter()
            .map(|path| path.display().to_string())
            .collect()
    }
}

impl ToolWriteGuard for WriteLeaseManager {
    fn assert_write_paths(&self, agent_id: Option<&AgentId>, paths: &[PathBuf]) -> ToolResult<()> {
        let Some(agent_id) = agent_id else {
            return Ok(());
        };
        let requested = paths
            .iter()
            .map(|path| normalize_path(path))
            .collect::<Vec<_>>();
        let guard = self.leases.read().expect("write lease lock");
        for path in &requested {
            if let Some((owned_path, owner)) = guard.find_conflict(agent_id, path) {
                return Err(ToolError::invalid_state(format!(
                    "write lease conflict for {}: owned by {} via {}",
                    path.display(),
                    owner,
                    owned_path.display()
                )));
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct LeaseIndex {
    owners: BTreeMap<AgentId, BTreeSet<PathBuf>>,
    root: LeaseTrieNode,
}

impl LeaseIndex {
    fn find_conflict(&self, agent_id: &AgentId, path: &Path) -> Option<(PathBuf, AgentId)> {
        self.root.find_conflict(agent_id, &path_components(path))
    }

    fn insert(&mut self, agent_id: &AgentId, path: PathBuf) {
        if !self
            .owners
            .entry(agent_id.clone())
            .or_default()
            .insert(path.clone())
        {
            return;
        }

        self.root.insert(agent_id, &path_components(&path));
    }

    fn release(&mut self, agent_id: &AgentId) {
        let Some(paths) = self.owners.remove(agent_id) else {
            return;
        };

        for path in paths {
            self.root.remove(agent_id, &path_components(&path));
        }
    }

    fn claimed_paths(&self, agent_id: &AgentId) -> Vec<PathBuf> {
        self.owners
            .get(agent_id)
            .map(|paths| paths.iter().cloned().collect())
            .unwrap_or_default()
    }
}

#[derive(Default)]
struct LeaseTrieNode {
    owner: Option<AgentId>,
    // Keep per-owner subtree counts so conflict checks can prune unrelated branches
    // without scanning every active lease in the global index.
    subtree_owner_counts: BTreeMap<AgentId, usize>,
    children: BTreeMap<OsString, LeaseTrieNode>,
}

impl LeaseTrieNode {
    fn find_conflict(
        &self,
        agent_id: &AgentId,
        components: &[OsString],
    ) -> Option<(PathBuf, AgentId)> {
        let mut node = self;
        let mut prefix = PathBuf::new();

        for component in components {
            if let Some(owner) = &node.owner
                && owner != agent_id
            {
                return Some((prefix.clone(), owner.clone()));
            }

            let Some(child) = node.children.get(component) else {
                return None;
            };
            prefix.push(component);
            node = child;
        }

        if let Some(owner) = &node.owner
            && owner != agent_id
        {
            return Some((prefix, owner.clone()));
        }

        node.find_conflicting_descendant(agent_id, &prefix)
    }

    fn find_conflicting_descendant(
        &self,
        agent_id: &AgentId,
        prefix: &Path,
    ) -> Option<(PathBuf, AgentId)> {
        if !has_foreign_owner(&self.subtree_owner_counts, agent_id) {
            return None;
        }

        for (component, child) in &self.children {
            if !has_foreign_owner(&child.subtree_owner_counts, agent_id) {
                continue;
            }

            let mut child_path = prefix.to_path_buf();
            child_path.push(component);
            if let Some(owner) = &child.owner
                && owner != agent_id
            {
                return Some((child_path, owner.clone()));
            }

            if let Some(conflict) = child.find_conflicting_descendant(agent_id, &child_path) {
                return Some(conflict);
            }
        }

        None
    }

    fn insert(&mut self, agent_id: &AgentId, components: &[OsString]) {
        *self
            .subtree_owner_counts
            .entry(agent_id.clone())
            .or_default() += 1;
        if components.is_empty() {
            self.owner = Some(agent_id.clone());
            return;
        }

        self.children
            .entry(components[0].clone())
            .or_default()
            .insert(agent_id, &components[1..]);
    }

    fn remove(&mut self, agent_id: &AgentId, components: &[OsString]) -> bool {
        decrement_owner_count(&mut self.subtree_owner_counts, agent_id);

        if components.is_empty() {
            self.owner = None;
            return self.owner.is_none()
                && self.children.is_empty()
                && self.subtree_owner_counts.is_empty();
        }

        let should_prune_child = self
            .children
            .get_mut(&components[0])
            .map(|child| child.remove(agent_id, &components[1..]))
            .unwrap_or(false);

        if should_prune_child {
            self.children.remove(&components[0]);
        }

        self.owner.is_none() && self.children.is_empty() && self.subtree_owner_counts.is_empty()
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn path_components(path: &Path) -> Vec<OsString> {
    path.components()
        .map(|component| component.as_os_str().to_os_string())
        .collect()
}

fn has_foreign_owner(owner_counts: &BTreeMap<AgentId, usize>, agent_id: &AgentId) -> bool {
    owner_counts
        .iter()
        .any(|(owner, count)| owner != agent_id && *count > 0)
}

fn decrement_owner_count(owner_counts: &mut BTreeMap<AgentId, usize>, agent_id: &AgentId) {
    let should_remove = match owner_counts.get_mut(agent_id) {
        Some(count) if *count > 1 => {
            *count -= 1;
            false
        }
        Some(_) => true,
        None => false,
    };

    if should_remove {
        owner_counts.remove(agent_id);
    }
}

#[cfg(test)]
mod tests {
    use super::WriteLeaseManager;
    use tools::ToolWriteGuard;
    use types::AgentId;

    #[test]
    fn claim_rejects_nested_conflict() {
        let manager = WriteLeaseManager::new();
        manager
            .claim(
                &AgentId::from("agent_a"),
                &[std::path::PathBuf::from("/repo/src")],
            )
            .unwrap();

        let conflict = manager
            .claim(
                &AgentId::from("agent_b"),
                &[std::path::PathBuf::from("/repo/src/lib.rs")],
            )
            .unwrap_err();
        assert_eq!(conflict.owner, AgentId::from("agent_a"));
    }

    #[test]
    fn write_guard_rejects_other_agent_writes() {
        let manager = WriteLeaseManager::new();
        manager
            .claim(
                &AgentId::from("agent_a"),
                &[std::path::PathBuf::from("/repo/src/lib.rs")],
            )
            .unwrap();
        let error = manager
            .assert_write_paths(
                Some(&AgentId::from("agent_b")),
                &[std::path::PathBuf::from("/repo/src/lib.rs")],
            )
            .unwrap_err();
        assert!(error.to_string().contains("write lease conflict"));
    }

    #[test]
    fn claim_rejects_same_path_conflict() {
        let manager = WriteLeaseManager::new();
        manager
            .claim(
                &AgentId::from("agent_a"),
                &[std::path::PathBuf::from("/repo/src/lib.rs")],
            )
            .unwrap();

        let conflict = manager
            .claim(
                &AgentId::from("agent_b"),
                &[std::path::PathBuf::from("/repo/src/lib.rs")],
            )
            .unwrap_err();
        assert_eq!(conflict.owner_path, "/repo/src/lib.rs");
    }

    #[test]
    fn claim_allows_non_conflicting_paths() {
        let manager = WriteLeaseManager::new();
        manager
            .claim(
                &AgentId::from("agent_a"),
                &[std::path::PathBuf::from("/repo/src/lib.rs")],
            )
            .unwrap();

        manager
            .claim(
                &AgentId::from("agent_b"),
                &[std::path::PathBuf::from("/repo/tests/lib.rs")],
            )
            .unwrap();

        assert_eq!(
            manager.claimed_paths(&AgentId::from("agent_b")),
            vec!["/repo/tests/lib.rs".to_string()]
        );
    }

    #[test]
    fn release_clears_nested_claims_for_same_agent() {
        let manager = WriteLeaseManager::new();
        let owner = AgentId::from("agent_a");
        manager
            .claim(
                &owner,
                &[
                    std::path::PathBuf::from("/repo/src"),
                    std::path::PathBuf::from("/repo/src/lib.rs"),
                ],
            )
            .unwrap();

        manager.release(&owner);

        manager
            .claim(
                &AgentId::from("agent_b"),
                &[std::path::PathBuf::from("/repo/src/lib.rs")],
            )
            .unwrap();
    }

    #[test]
    fn large_sparse_claim_set_preserves_local_conflict_checks() {
        let manager = WriteLeaseManager::new();
        for index in 0..512 {
            manager
                .claim(
                    &AgentId::from(format!("agent_{index}")),
                    &[std::path::PathBuf::from(format!("/repo/dir_{index}"))],
                )
                .unwrap();
        }

        let conflict = manager
            .claim(
                &AgentId::from("agent_z"),
                &[std::path::PathBuf::from("/repo/dir_320/file.rs")],
            )
            .unwrap_err();
        assert_eq!(conflict.owner, AgentId::from("agent_320"));

        manager
            .claim(
                &AgentId::from("agent_ok"),
                &[std::path::PathBuf::from("/repo/fresh/file.rs")],
            )
            .unwrap();
    }
}
