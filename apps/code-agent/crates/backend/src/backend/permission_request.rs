use crate::frontend_contract::permission_request_prompt_from_request;
use crate::interaction::{PermissionRequestDecision, PermissionRequestPrompt};
use agent::new_opaque_id;
use agent::runtime::PermissionGrantStore;
use agent::tools::{
    GrantedPermissionResponse, PermissionRequest, PermissionRequestHandler, Result as ToolResult,
    ToolError,
};
use async_trait::async_trait;
use std::sync::{Arc, RwLock};
use tokio::sync::oneshot;

#[derive(Default)]
struct PermissionRequestCoordinatorState {
    prompt: Option<PermissionRequestPrompt>,
    requested_permissions: Option<agent::tools::GrantedPermissionProfile>,
    responder: Option<oneshot::Sender<ToolResult<GrantedPermissionResponse>>>,
}

#[derive(Clone, Default)]
pub struct PermissionRequestCoordinator {
    inner: Arc<RwLock<PermissionRequestCoordinatorState>>,
}

impl PermissionRequestCoordinator {
    pub fn snapshot(&self) -> Option<PermissionRequestPrompt> {
        self.inner.read().unwrap().prompt.clone()
    }

    pub fn resolve(&self, decision: PermissionRequestDecision) -> bool {
        let mut inner = self.inner.write().unwrap();
        let responder = inner.responder.take();
        let requested_permissions = inner.requested_permissions.take();
        inner.prompt = None;
        if let Some(responder) = responder {
            let response = match decision {
                PermissionRequestDecision::GrantOnce => GrantedPermissionResponse {
                    permissions: requested_permissions.unwrap_or_default(),
                    scope: agent::tools::PermissionGrantScope::Turn,
                },
                PermissionRequestDecision::GrantForSession => GrantedPermissionResponse {
                    permissions: requested_permissions.unwrap_or_default(),
                    scope: agent::tools::PermissionGrantScope::Session,
                },
                PermissionRequestDecision::Deny => GrantedPermissionResponse {
                    permissions: agent::tools::GrantedPermissionProfile::default(),
                    scope: agent::tools::PermissionGrantScope::Turn,
                },
            };
            let _ = responder.send(Ok(response));
            true
        } else {
            false
        }
    }

    pub fn cancel(&self, reason: impl Into<String>) -> bool {
        let mut inner = self.inner.write().unwrap();
        let responder = inner.responder.take();
        inner.prompt = None;
        inner.requested_permissions = None;
        if let Some(responder) = responder {
            let _ = responder.send(Err(ToolError::invalid_state(reason.into())));
            true
        } else {
            false
        }
    }

    fn present(
        &self,
        prompt: PermissionRequestPrompt,
        requested_permissions: agent::tools::GrantedPermissionProfile,
        responder: oneshot::Sender<ToolResult<GrantedPermissionResponse>>,
    ) {
        let mut inner = self.inner.write().unwrap();
        inner.prompt = Some(prompt);
        inner.requested_permissions = Some(requested_permissions);
        inner.responder = Some(responder);
    }
}

pub struct SessionPermissionRequestHandler {
    coordinator: PermissionRequestCoordinator,
    grants: PermissionGrantStore,
}

impl SessionPermissionRequestHandler {
    pub fn new(coordinator: PermissionRequestCoordinator, grants: PermissionGrantStore) -> Self {
        Self {
            coordinator,
            grants,
        }
    }
}

#[async_trait]
impl PermissionRequestHandler for SessionPermissionRequestHandler {
    async fn request_permissions(
        &self,
        request: PermissionRequest,
    ) -> ToolResult<GrantedPermissionResponse> {
        let snapshot = self.grants.snapshot();
        let (tx, rx) = oneshot::channel();
        self.coordinator.present(
            permission_request_prompt_from_request(
                new_opaque_id().to_string(),
                &request,
                &snapshot,
            ),
            request.permissions,
            tx,
        );
        match rx.await {
            Ok(result) => {
                let result = result?;
                self.grants.grant(result.scope, &result.permissions);
                Ok(result)
            }
            Err(error) => Err(ToolError::invalid_state(format!(
                "request_permissions dialog closed unexpectedly: {error}"
            ))),
        }
    }
}

pub struct NonInteractivePermissionRequestHandler {
    reason: String,
}

impl NonInteractivePermissionRequestHandler {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

#[async_trait]
impl PermissionRequestHandler for NonInteractivePermissionRequestHandler {
    async fn request_permissions(
        &self,
        _request: PermissionRequest,
    ) -> ToolResult<GrantedPermissionResponse> {
        Err(ToolError::invalid_state(self.reason.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::{PermissionRequestCoordinator, SessionPermissionRequestHandler};
    use crate::interaction::PermissionRequestDecision;
    use agent::runtime::PermissionGrantStore;
    use agent::tools::{
        GrantedFilesystemPermissions, GrantedPermissionProfile, PermissionRequest,
        PermissionRequestHandler,
    };
    use tokio::task::yield_now;

    #[tokio::test]
    async fn granted_permissions_are_recorded_in_shared_store() {
        let coordinator = PermissionRequestCoordinator::default();
        let grants = PermissionGrantStore::default();
        let handler = SessionPermissionRequestHandler::new(coordinator.clone(), grants.clone());

        let task = tokio::spawn(async move {
            handler
                .request_permissions(PermissionRequest {
                    reason: Some("need write".to_string()),
                    permissions: GrantedPermissionProfile {
                        file_system: GrantedFilesystemPermissions {
                            read_roots: Vec::new(),
                            write_roots: vec!["/tmp/project".into()],
                        },
                        network: None,
                    },
                })
                .await
        });

        let prompt = loop {
            if let Some(prompt) = coordinator.snapshot() {
                break prompt;
            }
            yield_now().await;
        };
        assert_eq!(prompt.requested.write_roots.len(), 1);
        assert!(coordinator.resolve(PermissionRequestDecision::GrantForSession));

        let response = task.await.unwrap().unwrap();
        assert_eq!(response.scope, agent::tools::PermissionGrantScope::Session);
        assert_eq!(grants.snapshot().session.file_system.write_roots.len(), 1);
    }
}
