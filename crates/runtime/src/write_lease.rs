use std::collections::BTreeMap;
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
    leases: Arc<RwLock<BTreeMap<PathBuf, AgentId>>>,
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
            if let Some((owned_path, owner)) = guard.iter().find(|(owned_path, owner)| {
                **owner != *agent_id && paths_conflict(owned_path, path)
            }) {
                return Err(WriteLeaseConflict {
                    requested: path.display().to_string(),
                    owner: owner.clone(),
                    owner_path: owned_path.display().to_string(),
                });
            }
        }
        for path in requested {
            guard.insert(path, agent_id.clone());
        }
        Ok(())
    }

    pub fn release(&self, agent_id: &AgentId) {
        self.leases
            .write()
            .expect("write lease lock")
            .retain(|_, owner| owner != agent_id);
    }

    #[must_use]
    pub fn claimed_paths(&self, agent_id: &AgentId) -> Vec<String> {
        self.leases
            .read()
            .expect("write lease lock")
            .iter()
            .filter(|(_, owner)| *owner == agent_id)
            .map(|(path, _)| path.display().to_string())
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
            if let Some((owned_path, owner)) = guard.iter().find(|(owned_path, owner)| {
                **owner != *agent_id && paths_conflict(owned_path, path)
            }) {
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

fn paths_conflict(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
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
}
