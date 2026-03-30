use crate::{Result, RuntimeError};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tools::{
    FilesystemPolicy, HostEscapePolicy, NetworkPolicy, SandboxMode, SandboxPolicy,
    ToolExecutionContext,
};
use tracing::{debug, warn};
use types::{HookExecutionPolicy, HookNetworkPolicy, HookRegistration};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HookAuditAction {
    ExecutePath,
    NetworkRequest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HookAuditOutcome {
    Allowed,
    Denied { reason: String },
    Completed,
    Failed { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HookAuditEvent {
    pub hook_name: String,
    pub plugin_id: Option<String>,
    pub handler_kind: &'static str,
    pub action: HookAuditAction,
    pub target: String,
    pub outcome: HookAuditOutcome,
}

pub(crate) trait HookExecutionObserver: Send + Sync {
    fn record(&self, event: HookAuditEvent);
}

#[derive(Clone, Default)]
pub(crate) struct TracingHookExecutionObserver;

impl HookExecutionObserver for TracingHookExecutionObserver {
    fn record(&self, event: HookAuditEvent) {
        let plugin_id = event.plugin_id.as_deref().unwrap_or("unknown");
        match &event.outcome {
            HookAuditOutcome::Allowed => debug!(
                hook_name = %event.hook_name,
                plugin_id = %plugin_id,
                handler = event.handler_kind,
                action = ?event.action,
                target = %event.target,
                "hook execution request authorized",
            ),
            HookAuditOutcome::Denied { reason } => warn!(
                hook_name = %event.hook_name,
                plugin_id = %plugin_id,
                handler = event.handler_kind,
                action = ?event.action,
                target = %event.target,
                reason = %reason,
                "hook execution request denied",
            ),
            HookAuditOutcome::Completed => debug!(
                hook_name = %event.hook_name,
                plugin_id = %plugin_id,
                handler = event.handler_kind,
                action = ?event.action,
                target = %event.target,
                "hook execution completed",
            ),
            HookAuditOutcome::Failed { reason } => warn!(
                hook_name = %event.hook_name,
                plugin_id = %plugin_id,
                handler = event.handler_kind,
                action = ?event.action,
                target = %event.target,
                reason = %reason,
                "hook execution failed",
            ),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AuthorizedHookExecution {
    pub tool_context: ToolExecutionContext,
}

pub(crate) fn authorize_execute_path(
    registration: &HookRegistration,
    handler_kind: &'static str,
    path: &Path,
    observer: &dyn HookExecutionObserver,
) -> Result<AuthorizedHookExecution> {
    let target = path.display().to_string();
    let policy = match required_execution_policy(registration) {
        Ok(policy) => policy,
        Err(error) => {
            record_denied(
                observer,
                registration,
                handler_kind,
                HookAuditAction::ExecutePath,
                target,
                &error,
            );
            return Err(error);
        }
    };
    let tool_context = tool_context_for_execution(policy);
    match tool_context.assert_path_execute_allowed(path) {
        Ok(()) => {
            record_allowed(
                observer,
                registration,
                handler_kind,
                HookAuditAction::ExecutePath,
                target,
            );
            Ok(AuthorizedHookExecution { tool_context })
        }
        Err(error) => {
            let error = RuntimeError::hook(error.to_string());
            record_denied(
                observer,
                registration,
                handler_kind,
                HookAuditAction::ExecutePath,
                target,
                &error,
            );
            Err(error)
        }
    }
}

pub(crate) fn authorize_network_url(
    registration: &HookRegistration,
    handler_kind: &'static str,
    url: &str,
    observer: &dyn HookExecutionObserver,
) -> Result<AuthorizedHookExecution> {
    let target = url.to_string();
    let policy = match required_execution_policy(registration) {
        Ok(policy) => policy,
        Err(error) => {
            record_denied(
                observer,
                registration,
                handler_kind,
                HookAuditAction::NetworkRequest,
                target,
                &error,
            );
            return Err(error);
        }
    };
    let tool_context = tool_context_for_execution(policy);
    match assert_network_allowed(&tool_context, url) {
        Ok(()) => {
            record_allowed(
                observer,
                registration,
                handler_kind,
                HookAuditAction::NetworkRequest,
                target,
            );
            Ok(AuthorizedHookExecution { tool_context })
        }
        Err(error) => {
            record_denied(
                observer,
                registration,
                handler_kind,
                HookAuditAction::NetworkRequest,
                target,
                &error,
            );
            Err(error)
        }
    }
}

pub(crate) fn record_completion(
    observer: &dyn HookExecutionObserver,
    registration: &HookRegistration,
    handler_kind: &'static str,
    action: HookAuditAction,
    target: impl Into<String>,
) {
    observer.record(HookAuditEvent {
        hook_name: registration.name.to_string(),
        plugin_id: registration
            .execution
            .as_ref()
            .and_then(|execution| execution.plugin_id.clone().map(|id| id.to_string())),
        handler_kind,
        action,
        target: target.into(),
        outcome: HookAuditOutcome::Completed,
    });
}

pub(crate) fn record_failure(
    observer: &dyn HookExecutionObserver,
    registration: &HookRegistration,
    handler_kind: &'static str,
    action: HookAuditAction,
    target: impl Into<String>,
    error: &RuntimeError,
) {
    observer.record(HookAuditEvent {
        hook_name: registration.name.to_string(),
        plugin_id: registration
            .execution
            .as_ref()
            .and_then(|execution| execution.plugin_id.clone().map(|id| id.to_string())),
        handler_kind,
        action,
        target: target.into(),
        outcome: HookAuditOutcome::Failed {
            reason: error.to_string(),
        },
    });
}

pub(crate) fn tool_context_for_execution(execution: &HookExecutionPolicy) -> ToolExecutionContext {
    let workspace_root = execution
        .plugin_root
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    ToolExecutionContext {
        workspace_root: workspace_root.clone(),
        worktree_root: Some(workspace_root),
        read_only_roots: execution.read_roots.clone(),
        writable_roots: execution.write_roots.clone(),
        exec_roots: execution.exec_roots.clone(),
        network_policy: Some(match &execution.network {
            HookNetworkPolicy::Deny => NetworkPolicy::Off,
            HookNetworkPolicy::Allow => NetworkPolicy::Full,
            HookNetworkPolicy::AllowDomains { domains } => {
                NetworkPolicy::AllowDomains(domains.clone())
            }
        }),
        workspace_only: true,
        ..Default::default()
    }
}

pub(crate) fn tighten_hook_sandbox_policy(
    base: &SandboxPolicy,
    tool_context: &ToolExecutionContext,
) -> SandboxPolicy {
    let derived = tool_context.sandbox_policy();
    SandboxPolicy {
        mode: stricter_mode(&base.mode, &derived.mode),
        filesystem: FilesystemPolicy {
            readable_roots: intersect_path_roots(
                &base.filesystem.readable_roots,
                &derived.filesystem.readable_roots,
            ),
            writable_roots: intersect_path_roots(
                &base.filesystem.writable_roots,
                &derived.filesystem.writable_roots,
            ),
            executable_roots: intersect_path_roots(
                &base.filesystem.executable_roots,
                &derived.filesystem.executable_roots,
            ),
            protected_paths: union_paths(
                &base.filesystem.protected_paths,
                &derived.filesystem.protected_paths,
            ),
        },
        network: intersect_network_policy(&base.network, &derived.network),
        host_escape: stricter_host_escape(&base.host_escape, &derived.host_escape),
        fail_if_unavailable: base.fail_if_unavailable || derived.fail_if_unavailable,
    }
}

fn required_execution_policy(registration: &HookRegistration) -> Result<&HookExecutionPolicy> {
    registration.execution.as_ref().ok_or_else(|| {
        RuntimeError::hook(format!(
            "hook `{}` requires execution policy grants",
            registration.name
        ))
    })
}

fn assert_network_allowed(tool_context: &ToolExecutionContext, url: &str) -> Result<()> {
    match tool_context
        .network_policy
        .as_ref()
        .unwrap_or(&NetworkPolicy::Off)
    {
        NetworkPolicy::Off => Err(RuntimeError::hook(format!(
            "hook network access denied for url `{url}`"
        ))),
        NetworkPolicy::Full => Ok(()),
        NetworkPolicy::AllowDomains(domains) => {
            let host = reqwest::Url::parse(url)
                .map_err(|error| RuntimeError::hook(format!("invalid hook HTTP url: {error}")))?
                .host_str()
                .ok_or_else(|| RuntimeError::hook("hook HTTP url missing host"))?
                .to_string();
            if domains.iter().any(|domain| domain == &host) {
                Ok(())
            } else {
                Err(RuntimeError::hook(format!(
                    "hook HTTP url `{url}` is outside granted domains"
                )))
            }
        }
    }
}

fn record_allowed(
    observer: &dyn HookExecutionObserver,
    registration: &HookRegistration,
    handler_kind: &'static str,
    action: HookAuditAction,
    target: String,
) {
    observer.record(HookAuditEvent {
        hook_name: registration.name.to_string(),
        plugin_id: registration
            .execution
            .as_ref()
            .and_then(|execution| execution.plugin_id.clone().map(|id| id.to_string())),
        handler_kind,
        action,
        target,
        outcome: HookAuditOutcome::Allowed,
    });
}

fn record_denied(
    observer: &dyn HookExecutionObserver,
    registration: &HookRegistration,
    handler_kind: &'static str,
    action: HookAuditAction,
    target: String,
    error: &RuntimeError,
) {
    observer.record(HookAuditEvent {
        hook_name: registration.name.to_string(),
        plugin_id: registration
            .execution
            .as_ref()
            .and_then(|execution| execution.plugin_id.clone().map(|id| id.to_string())),
        handler_kind,
        action,
        target,
        outcome: HookAuditOutcome::Denied {
            reason: error.to_string(),
        },
    });
}

fn stricter_mode(left: &SandboxMode, right: &SandboxMode) -> SandboxMode {
    match (left, right) {
        (SandboxMode::ReadOnly, _) | (_, SandboxMode::ReadOnly) => SandboxMode::ReadOnly,
        (SandboxMode::WorkspaceWrite, _) | (_, SandboxMode::WorkspaceWrite) => {
            SandboxMode::WorkspaceWrite
        }
        (SandboxMode::DangerFullAccess, SandboxMode::DangerFullAccess) => {
            SandboxMode::DangerFullAccess
        }
    }
}

fn stricter_host_escape(left: &HostEscapePolicy, right: &HostEscapePolicy) -> HostEscapePolicy {
    match (left, right) {
        (HostEscapePolicy::Deny, _) | (_, HostEscapePolicy::Deny) => HostEscapePolicy::Deny,
        (HostEscapePolicy::HostManaged, HostEscapePolicy::HostManaged) => {
            HostEscapePolicy::HostManaged
        }
    }
}

fn intersect_network_policy(left: &NetworkPolicy, right: &NetworkPolicy) -> NetworkPolicy {
    match (left, right) {
        (NetworkPolicy::Off, _) | (_, NetworkPolicy::Off) => NetworkPolicy::Off,
        (NetworkPolicy::Full, policy) | (policy, NetworkPolicy::Full) => policy.clone(),
        (NetworkPolicy::AllowDomains(left_domains), NetworkPolicy::AllowDomains(right_domains)) => {
            let allowed = left_domains
                .iter()
                .filter(|domain| right_domains.contains(*domain))
                .cloned()
                .collect::<Vec<_>>();
            if allowed.is_empty() {
                NetworkPolicy::Off
            } else {
                NetworkPolicy::AllowDomains(allowed)
            }
        }
    }
}

fn intersect_path_roots(left: &[PathBuf], right: &[PathBuf]) -> Vec<PathBuf> {
    if left.is_empty() {
        return union_paths(&[], right);
    }
    if right.is_empty() {
        return union_paths(left, &[]);
    }

    let mut overlap = BTreeSet::new();
    for left_path in left {
        for right_path in right {
            if right_path.starts_with(left_path) {
                overlap.insert(right_path.clone());
            } else if left_path.starts_with(right_path) {
                overlap.insert(left_path.clone());
            }
        }
    }
    overlap.into_iter().collect()
}

fn union_paths(left: &[PathBuf], right: &[PathBuf]) -> Vec<PathBuf> {
    left.iter()
        .chain(right.iter())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
