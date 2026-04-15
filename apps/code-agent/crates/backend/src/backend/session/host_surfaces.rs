use super::*;
use code_agent_config::filter_unavailable_builtin_mcp_servers;

impl CodeAgentSession {
    fn configured_stdio_mcp_server_names(&self) -> Vec<String> {
        self.configured_mcp_servers
            .read()
            .unwrap()
            .iter()
            .filter_map(|server| match server.transport {
                McpTransportConfig::Stdio { .. } => Some(server.name.to_string()),
                McpTransportConfig::StreamableHttp { .. } => None,
            })
            .collect()
    }

    fn pending_stdio_mcp_server_configs(&self) -> Vec<McpServerConfig> {
        let connected_names = self
            .mcp_servers
            .read()
            .unwrap()
            .iter()
            .map(|server| server.server_name.clone())
            .collect::<BTreeSet<_>>();
        self.configured_mcp_servers
            .read()
            .unwrap()
            .iter()
            .filter(|server| matches!(server.transport, McpTransportConfig::Stdio { .. }))
            .filter(|server| !connected_names.contains(&server.name))
            .cloned()
            .collect()
    }

    fn configured_command_hook_names(&self) -> Vec<String> {
        self.configured_runtime_hooks
            .read()
            .unwrap()
            .iter()
            .filter_map(|hook| match hook.handler {
                HookHandler::Command(_) => Some(hook.name.to_string()),
                _ => None,
            })
            .collect()
    }

    fn runtime_hooks_for_host_process_mode(
        &self,
        host_process_surfaces_allowed: bool,
    ) -> Vec<HookRegistration> {
        if host_process_surfaces_allowed {
            return self.configured_runtime_hooks.read().unwrap().clone();
        }

        self.configured_runtime_hooks
            .read()
            .unwrap()
            .iter()
            .filter(|hook| !matches!(hook.handler, HookHandler::Command(_)))
            .cloned()
            .collect()
    }

    pub(super) fn refresh_startup_diagnostics_snapshot(
        &self,
        runtime: &AgentRuntime,
        host_process_surfaces_allowed: bool,
        host_process_block_reason: Option<&str>,
    ) -> StartupDiagnosticsSnapshot {
        let mut snapshot = self.startup.read().unwrap().startup_diagnostics.clone();
        let tool_specs = runtime.tool_specs();
        let local_tool_count = tool_specs
            .iter()
            .filter(|tool| matches!(tool.origin, agent::types::ToolOrigin::Local))
            .count();
        snapshot.local_tool_count = local_tool_count;
        snapshot.mcp_tool_count = tool_specs.len().saturating_sub(local_tool_count);
        snapshot.mcp_servers = list_mcp_servers(&self.connected_mcp_servers_snapshot());
        snapshot.warnings.retain(|warning| {
            !warning.starts_with(STDIO_MCP_DISABLED_WARNING_PREFIX)
                && !warning.starts_with(COMMAND_HOOK_DISABLED_WARNING_PREFIX)
                && !warning.starts_with(MANAGED_CODE_INTEL_DISABLED_WARNING_PREFIX)
        });
        if !host_process_surfaces_allowed {
            let blocked = self.configured_stdio_mcp_server_names();
            if !blocked.is_empty() {
                snapshot.warnings.push(format!(
                    "{STDIO_MCP_DISABLED_WARNING_PREFIX} {}",
                    blocked.join(", ")
                ));
            }
            let blocked_hooks = self.configured_command_hook_names();
            if !blocked_hooks.is_empty() {
                snapshot.warnings.push(format!(
                    "{COMMAND_HOOK_DISABLED_WARNING_PREFIX} {}",
                    blocked_hooks.join(", ")
                ));
            }
            if self.code_intel_backend.managed_helpers_supported() {
                let reason = host_process_block_reason.unwrap_or("backend unavailable");
                snapshot.warnings.push(format!(
                    "{MANAGED_CODE_INTEL_DISABLED_WARNING_PREFIX} {reason}"
                ));
            }
        }
        snapshot
    }

    pub(super) async fn connect_pending_stdio_mcp_servers(
        &self,
        sandbox_policy: &SandboxPolicy,
    ) -> Result<Vec<(ConnectedMcpServer, Vec<McpToolAdapter>)>> {
        let mut warnings = Vec::new();
        let pending_configs = filter_unavailable_builtin_mcp_servers(
            &self.managed_surface_reload.env_map,
            self.pending_stdio_mcp_server_configs(),
            &mut warnings,
        );
        if !warnings.is_empty() {
            let mut startup = self.startup.write().unwrap();
            for warning in warnings {
                if !startup.startup_diagnostics.warnings.contains(&warning) {
                    startup.startup_diagnostics.warnings.push(warning);
                }
            }
        }
        if pending_configs.is_empty() {
            return Ok(Vec::new());
        }

        let connected = connect_and_catalog_mcp_servers_with_options(
            &pending_configs,
            McpConnectOptions {
                process_executor: self.mcp_process_executor.clone(),
                sandbox_policy: sandbox_policy.clone(),
                ..Default::default()
            },
        )
        .await?;
        let mut prepared = Vec::with_capacity(connected.len());
        for server in connected {
            let adapters = catalog_tools_as_registry_entries(server.client.clone()).await?;
            prepared.push((server, adapters));
        }
        Ok(prepared)
    }

    pub(super) fn set_runtime_hooks(
        &self,
        runtime: &mut AgentRuntime,
        host_process_surfaces_allowed: bool,
    ) {
        let hooks = self.runtime_hooks_for_host_process_mode(host_process_surfaces_allowed);
        runtime.replace_hooks(hooks.clone());
        // Active child runtimes intentionally keep the hook list they launched
        // with. Permission-mode revocation is enforced at execution time by the
        // shared command executor, while future child runtimes pick up this
        // refreshed snapshot during spawn.
        *self.runtime_hooks.write().unwrap() = hooks;
    }

    pub(super) fn rebuild_mcp_resource_tools(&self, runtime: &mut AgentRuntime) {
        let registry = runtime.tool_registry_handle();
        // Resource listing/reading stays behind fixed aggregate tool names, so
        // permission-mode changes that add or remove servers must rebuild the
        // shared specs to keep per-server MCP boundary metadata accurate.
        for tool_name in MCP_RESOURCE_TOOL_NAMES {
            registry.remove(tool_name);
        }
        for resource_tool in
            catalog_resource_tools_as_registry_entries(self.connected_mcp_servers_snapshot())
        {
            let mut registry = registry.clone();
            registry.register(resource_tool);
        }
    }

    pub(super) fn attach_connected_stdio_mcp_servers(
        &self,
        runtime: &mut AgentRuntime,
        connected_servers: Vec<(ConnectedMcpServer, Vec<McpToolAdapter>)>,
    ) {
        if connected_servers.is_empty() {
            return;
        }

        let registry = runtime.tool_registry_handle();
        let mut attached_server_names = Vec::new();
        {
            let mut current_servers = self.mcp_servers.write().unwrap();
            for (server, adapters) in connected_servers {
                // Stdio MCP servers were intentionally skipped at boot when the
                // host could not service local subprocesses. Once the session
                // enables that capability, register their per-server tools and
                // fold them back into the shared MCP resource surfaces.
                attached_server_names.push(server.server_name.to_string());
                for adapter in adapters {
                    let mut registry = registry.clone();
                    registry.register(adapter);
                }
                current_servers.push(server);
            }
        }
        self.rebuild_mcp_resource_tools(runtime);
        info!(
            "connected deferred stdio MCP servers after permission-mode change: {}",
            attached_server_names.join(", ")
        );
    }

    pub(super) fn detach_local_stdio_mcp_servers(&self, runtime: &mut AgentRuntime) {
        let removed_servers = {
            let mut current_servers = self.mcp_servers.write().unwrap();
            let (retained, removed): (Vec<_>, Vec<_>) =
                current_servers.drain(..).partition(|server| {
                    !matches!(
                        server.boundary.transport,
                        agent::types::McpTransportKind::Stdio
                    )
                });
            *current_servers = retained;
            removed
        };
        if removed_servers.is_empty() {
            return;
        }

        let registry = runtime.tool_registry_handle();
        let removed_server_names = removed_servers
            .iter()
            .map(|server| server.server_name.to_string())
            .collect::<Vec<_>>();
        for server in &removed_servers {
            for tool in &server.catalog.tools {
                registry.remove(tool.name.as_str());
            }
        }
        self.rebuild_mcp_resource_tools(runtime);
        info!(
            "detached local stdio MCP servers after permission-mode change: {}",
            removed_server_names.join(", ")
        );
    }
}
