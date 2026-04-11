use super::CodeAgentSession;
use crate::frontend_contract::permission_profile_from_granted;
use crate::interaction::{PermissionProfile, SessionPermissionMode, SessionPermissionModeOutcome};
use agent::runtime::PermissionGrantSnapshot;
use agent::tools::{
    HOST_FEATURE_HOST_PROCESS_SURFACES, SandboxPolicy, describe_sandbox_policy,
    sandbox_backend_status,
};
use anyhow::Result;
use std::sync::atomic::Ordering;

impl CodeAgentSession {
    pub fn host_process_surfaces_allowed(&self) -> bool {
        self.startup.read().unwrap().host_process_surfaces_allowed
    }

    pub fn permission_mode(&self) -> SessionPermissionMode {
        self.startup.read().unwrap().permission_mode
    }

    fn sandbox_policy_for_mode(&self, mode: SessionPermissionMode) -> SandboxPolicy {
        match mode {
            SessionPermissionMode::Default => self.default_sandbox_policy.clone(),
            SessionPermissionMode::DangerFullAccess => SandboxPolicy::permissive()
                .with_fail_if_unavailable(self.default_sandbox_policy.fail_if_unavailable),
        }
    }

    pub async fn set_permission_mode(
        &self,
        mode: SessionPermissionMode,
    ) -> Result<SessionPermissionModeOutcome> {
        self.ensure_turn_idle_for_permission_switch()?;
        let previous = self.permission_mode();
        let policy = self.sandbox_policy_for_mode(mode);
        let backend_status = sandbox_backend_status(&policy);
        let sandbox_summary = describe_sandbox_policy(&policy, &backend_status);
        let host_process_block_reason = backend_status.reason().map(str::to_string);
        let host_process_surfaces_allowed =
            !policy.requires_enforcement() || backend_status.is_available();
        let connected_stdio_mcp_servers = if host_process_surfaces_allowed {
            self.connect_pending_stdio_mcp_servers(&policy).await?
        } else {
            Vec::new()
        };
        let (tool_names, side_question_context, startup_diagnostics) = {
            let mut runtime = self.runtime.lock().await;
            self.host_process_executor
                .set_host_process_surfaces(host_process_surfaces_allowed);
            self.command_hook_executor
                .set_host_process_surfaces(host_process_surfaces_allowed, policy.clone());
            self.code_intel_backend
                .set_managed_helpers_enabled(host_process_surfaces_allowed);
            self.set_runtime_hooks(&mut runtime, host_process_surfaces_allowed);
            if host_process_surfaces_allowed {
                self.attach_connected_stdio_mcp_servers(&mut runtime, connected_stdio_mcp_servers);
            } else {
                self.detach_local_stdio_mcp_servers(&mut runtime);
            }
            let mut visibility = runtime.tool_visibility_context_snapshot();
            visibility.set_feature_enabled(
                HOST_FEATURE_HOST_PROCESS_SURFACES,
                host_process_surfaces_allowed,
            );

            // Sticky `request_permissions` grants stay in the runtime-owned
            // grant store. This setter only swaps the session's base sandbox
            // mode so later tool calls and newly spawned subagents inherit the
            // same host-selected baseline.
            runtime.replace_tool_visibility_context(visibility);
            runtime.set_base_sandbox_policy(policy.clone());
            self.sync_runtime_session_refs(&runtime);
            (
                runtime.tool_registry_names(),
                Self::side_question_context_from_runtime(&runtime, None),
                self.refresh_startup_diagnostics_snapshot(
                    &runtime,
                    host_process_surfaces_allowed,
                    host_process_block_reason.as_deref(),
                ),
            )
        };
        self.store_side_question_context(side_question_context);
        {
            let mut tool_context = self.session_tool_context.write().unwrap();
            tool_context.effective_sandbox_policy = Some(policy);
            tool_context.model_visibility.set_feature_enabled(
                HOST_FEATURE_HOST_PROCESS_SURFACES,
                host_process_surfaces_allowed,
            );
        }
        {
            let mut startup = self.startup.write().unwrap();
            startup.permission_mode = mode;
            startup.sandbox_summary = sandbox_summary.clone();
            startup.host_process_surfaces_allowed = host_process_surfaces_allowed;
            startup.tool_names = tool_names;
            startup.startup_diagnostics = startup_diagnostics;
        }

        Ok(SessionPermissionModeOutcome {
            previous,
            current: mode,
            sandbox_summary,
            host_process_surfaces_allowed,
        })
    }

    fn ensure_turn_idle_for_permission_switch(&self) -> Result<()> {
        if self.runtime_turn_active.load(Ordering::Acquire) {
            return Err(anyhow::anyhow!(
                super::PERMISSION_MODE_SWITCH_BLOCKED_WHILE_TURN_RUNNING
            ));
        }
        Ok(())
    }

    pub fn permission_grant_snapshot(&self) -> PermissionGrantSnapshot {
        self.permission_grants.snapshot()
    }

    pub fn permission_grant_profiles(&self) -> (PermissionProfile, PermissionProfile) {
        let snapshot = self.permission_grant_snapshot();
        (
            permission_profile_from_granted(&snapshot.turn),
            permission_profile_from_granted(&snapshot.session),
        )
    }
}
