use crate::Result;
use std::sync::{Arc, RwLock};
use tools::{
    GrantedPermissionProfile, PermissionGrantScope, SandboxPolicy, apply_granted_permission_profile,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PermissionGrantSnapshot {
    pub turn: GrantedPermissionProfile,
    pub session: GrantedPermissionProfile,
}

impl PermissionGrantSnapshot {
    #[must_use]
    pub fn merged(&self) -> GrantedPermissionProfile {
        self.session.merged(&self.turn)
    }
}

#[derive(Clone, Default)]
pub struct PermissionGrantStore {
    inner: Arc<RwLock<PermissionGrantSnapshot>>,
}

impl PermissionGrantStore {
    #[must_use]
    pub fn snapshot(&self) -> PermissionGrantSnapshot {
        self.inner.read().unwrap().clone()
    }

    pub fn grant(&self, scope: PermissionGrantScope, permissions: &GrantedPermissionProfile) {
        let mut inner = self.inner.write().unwrap();
        match scope {
            PermissionGrantScope::Turn => inner.turn.merge_in_place(permissions),
            PermissionGrantScope::Session => inner.session.merge_in_place(permissions),
        }
    }

    pub fn replace(&self, scope: PermissionGrantScope, permissions: GrantedPermissionProfile) {
        let mut inner = self.inner.write().unwrap();
        match scope {
            PermissionGrantScope::Turn => inner.turn = permissions,
            PermissionGrantScope::Session => inner.session = permissions,
        }
    }

    pub fn clear_turn(&self) {
        self.inner.write().unwrap().turn = GrantedPermissionProfile::default();
    }

    pub fn clear_session(&self) {
        self.inner.write().unwrap().session = GrantedPermissionProfile::default();
    }

    pub fn clear_all(&self) {
        *self.inner.write().unwrap() = PermissionGrantSnapshot::default();
    }

    pub fn effective_sandbox_policy(&self, base: &SandboxPolicy) -> Result<SandboxPolicy> {
        apply_granted_permission_profile(base, &self.snapshot().merged()).map_err(|error| {
            crate::RuntimeError::invalid_state_with_source(
                "failed to apply granted permission profile",
                error,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::PermissionGrantStore;
    use tools::{
        GrantedFilesystemPermissions, GrantedNetworkPermissions, GrantedPermissionProfile,
        HostEscapePolicy, NetworkPolicy, PermissionGrantScope, SandboxMode, SandboxPolicy,
    };

    #[test]
    fn session_and_turn_grants_merge_into_effective_policy() {
        let store = PermissionGrantStore::default();
        store.grant(
            PermissionGrantScope::Session,
            &GrantedPermissionProfile {
                file_system: GrantedFilesystemPermissions {
                    read_roots: vec!["/tmp/shared".into()],
                    write_roots: Vec::new(),
                },
                network: None,
            },
        );
        store.grant(
            PermissionGrantScope::Turn,
            &GrantedPermissionProfile {
                file_system: GrantedFilesystemPermissions {
                    read_roots: Vec::new(),
                    write_roots: vec!["/tmp/shared".into()],
                },
                network: Some(GrantedNetworkPermissions::AllowDomains(vec![
                    "example.com".to_string(),
                ])),
            },
        );

        let policy = store
            .effective_sandbox_policy(&SandboxPolicy {
                mode: SandboxMode::ReadOnly,
                filesystem: tools::FilesystemPolicy {
                    readable_roots: vec!["/workspace".into()],
                    writable_roots: Vec::new(),
                    executable_roots: Vec::new(),
                    protected_paths: Vec::new(),
                },
                network: NetworkPolicy::Off,
                host_escape: HostEscapePolicy::Deny,
                fail_if_unavailable: false,
            })
            .unwrap();
        assert_eq!(policy.mode, SandboxMode::WorkspaceWrite);
        assert!(
            policy
                .filesystem
                .readable_roots
                .iter()
                .any(|path| path == "/tmp/shared")
        );
        assert!(
            policy
                .filesystem
                .writable_roots
                .iter()
                .any(|path| path == "/tmp/shared")
        );
        assert_eq!(
            policy.network,
            NetworkPolicy::AllowDomains(vec!["example.com".to_string()])
        );
    }
}
