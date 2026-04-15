use super::*;
use crate::backend::mcp_connection_sandbox_policy;
use crate::backend::{
    build_plugin_activation_plan, build_system_preamble, dedup_mcp_servers,
    merge_driver_host_inputs, resolve_mcp_servers, resolve_skill_roots,
};
use agent::plugins::discover_plugins;
use agent::runtime::UserMessageAugmentor;
use agent::{SkillCatalog, ToolRegistry};
use code_agent_config::{
    builtin_skill_root, filter_unavailable_builtin_mcp_servers, list_core_mcp_servers,
    list_managed_skill_details, set_core_mcp_server_enabled,
    set_managed_plugin_enabled as persist_managed_plugin_enabled,
    set_managed_skill_enabled as persist_managed_skill_enabled,
};
use nanoclaw_config::CoreConfig;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

impl CodeAgentSession {
    pub async fn list_managed_mcp_servers(&self) -> Result<Vec<ManagedMcpServerSummary>> {
        let mut summaries = self
            .configured_mcp_server_summaries(true)
            .into_iter()
            .map(|server| ManagedMcpServerSummary {
                name: server.server_name,
                transport: server.transport,
                enabled: server.enabled,
                connected: server.connected,
                tool_count: server.tool_count,
                prompt_count: server.prompt_count,
                resource_count: server.resource_count,
                status_detail: server.status_detail,
            })
            .collect::<Vec<_>>();
        summaries.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(summaries)
    }

    pub async fn list_managed_skills(&self) -> Result<Vec<ManagedSkillSummary>> {
        let mut summaries = Vec::new();
        for skill in list_managed_skill_details(self.workspace_root()).await? {
            summaries.push(ManagedSkillSummary {
                name: skill.skill_name,
                description: skill.description,
                path: relativize_to_workspace(self.workspace_root(), &skill.skill_path),
                enabled: skill.enabled,
                builtin: skill.builtin,
            });
        }
        summaries.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(summaries)
    }

    pub async fn list_managed_plugins(&self) -> Result<Vec<ManagedPluginSummary>> {
        let plugins = self.managed_surface_reload.plugins.read().unwrap().clone();
        let plugin_plan = build_plugin_activation_plan(self.workspace_root(), &plugins)?;
        let discovery = discover_plugins(&resolved_plugin_roots(self.workspace_root(), &plugins))?;
        let discovered = discovery
            .plugins
            .into_iter()
            .map(|plugin| (plugin.manifest.id.clone(), plugin))
            .collect::<BTreeMap<_, _>>();
        let mut summaries = plugin_plan
            .plugin_states
            .into_iter()
            .map(|state| {
                let discovered_plugin = discovered.get(&state.plugin_id);
                ManagedPluginSummary {
                    plugin_id: state.plugin_id.to_string(),
                    kind: discovered_plugin
                        .map(|plugin| plugin_kind_label(plugin.manifest.kind))
                        .unwrap_or_else(|| "unknown".to_string()),
                    path: discovered_plugin
                        .map(|plugin| {
                            relativize_to_workspace(self.workspace_root(), &plugin.root_dir)
                        })
                        .unwrap_or_default(),
                    enabled: state.enabled,
                    contribution_summary: plugin_contribution_summary(&state),
                }
            })
            .collect::<Vec<_>>();
        summaries.sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));
        Ok(summaries)
    }

    pub async fn set_managed_mcp_server_enabled(&self, name: &str, enabled: bool) -> Result<()> {
        self.ensure_turn_idle_for_managed_surface_refresh()?;
        set_core_mcp_server_enabled(self.workspace_root(), name, enabled)?;
        *self.configured_mcp_servers.write().unwrap() =
            list_core_mcp_servers(self.workspace_root())?;
        self.refresh_managed_surfaces().await
    }

    pub async fn set_managed_skill_enabled(&self, name: &str, enabled: bool) -> Result<()> {
        self.ensure_turn_idle_for_managed_surface_refresh()?;
        let artifact = persist_managed_skill_enabled(self.workspace_root(), name, enabled).await?;
        let mut disabled_builtin = self
            .managed_surface_reload
            .disabled_builtin_skills
            .write()
            .unwrap();
        if artifact.builtin {
            if enabled {
                disabled_builtin.remove(&artifact.skill_name);
            } else {
                disabled_builtin.insert(artifact.skill_name);
            }
        }
        drop(disabled_builtin);
        self.refresh_managed_surfaces().await
    }

    pub async fn set_managed_plugin_enabled(&self, plugin_id: &str, enabled: bool) -> Result<()> {
        self.ensure_turn_idle_for_managed_surface_refresh()?;
        persist_managed_plugin_enabled(self.workspace_root(), plugin_id, enabled)?;
        let refreshed = CoreConfig::load_from_dir(self.workspace_root())?;
        *self.managed_surface_reload.plugins.write().unwrap() = refreshed.plugins;
        self.refresh_managed_surfaces().await
    }

    fn ensure_turn_idle_for_managed_surface_refresh(&self) -> Result<()> {
        if self.runtime_turn_active.load(Ordering::Acquire) {
            return Err(anyhow::anyhow!(
                super::MANAGED_SURFACE_REFRESH_BLOCKED_WHILE_TURN_RUNNING
            ));
        }
        Ok(())
    }

    async fn refresh_managed_surfaces(&self) -> Result<()> {
        let plugins = self.managed_surface_reload.plugins.read().unwrap().clone();
        let plugin_plan = build_plugin_activation_plan(self.workspace_root(), &plugins)?;
        let skill_roots = resolve_skill_roots(
            &self.managed_surface_reload.skill_roots,
            self.workspace_root(),
            &plugin_plan,
        );
        let refreshed_catalog = filter_disabled_builtin_skills(
            self.workspace_root(),
            &self
                .managed_surface_reload
                .disabled_builtin_skills
                .read()
                .unwrap(),
            agent::skills::load_skill_roots(&skill_roots).await?,
        );
        self.preamble
            .skill_catalog
            .replace(refreshed_catalog.roots(), refreshed_catalog.all());

        let sandbox_policy = self.session_tool_context.read().unwrap().sandbox_policy();
        let host_process_surfaces_allowed = self.host_process_surfaces_allowed();
        let mut startup_warnings = Vec::new();
        let mut runtime = self.runtime.lock().await;
        let mut registry = runtime.tool_registry_handle();

        remove_plugin_tools(&registry);
        remove_named_tools(
            &registry,
            &self
                .applied_plugin_surfaces
                .read()
                .unwrap()
                .driver_tool_names,
        );

        let plugin_custom_tool_outcome = agent::register_plugin_custom_tools(
            &plugin_plan.custom_tool_activations,
            Some(self.host_process_executor.clone() as Arc<dyn agent::tools::ProcessExecutor>),
            &registry,
        )?;
        startup_warnings.extend(plugin_custom_tool_outcome.warnings);

        let driver_outcome = agent::activate_driver_requests(
            &plugin_plan.runtime_activations,
            self.workspace_root(),
            Some(self.store.clone()),
            &mut registry,
            agent::UnknownDriverPolicy::Error,
        )?;

        let merged = merge_driver_host_inputs(
            plugin_plan.hooks.clone(),
            plugin_plan.mcp_servers.clone(),
            plugin_plan.instructions.clone(),
            &driver_outcome,
        );
        let mut runtime_hooks = merged.runtime_hooks;
        let plugin_mcp_servers = merged.mcp_servers;
        let plugin_instructions = merged.instructions;
        runtime_hooks.extend(
            self.preamble
                .skill_catalog
                .all()
                .into_iter()
                .flat_map(|skill| skill.hooks.clone()),
        );
        *self.configured_runtime_hooks.write().unwrap() = runtime_hooks.clone();
        let filtered_runtime_hooks = filter_runtime_hooks_for_host_surfaces(
            runtime_hooks,
            host_process_surfaces_allowed,
            &mut startup_warnings,
        );
        runtime.replace_hooks(filtered_runtime_hooks.clone());
        *self.runtime_hooks.write().unwrap() = filtered_runtime_hooks;

        let managed_mcp = self.configured_mcp_servers.read().unwrap().clone();
        let resolved_mcp = dedup_mcp_servers(resolve_mcp_servers(
            &[managed_mcp, plugin_mcp_servers].concat(),
            self.workspace_root(),
        ));
        let available_mcp = filter_unavailable_builtin_mcp_servers(
            &self.managed_surface_reload.env_map,
            resolved_mcp,
            &mut startup_warnings,
        );
        let target_mcp = filter_mcp_servers_for_host_surfaces(
            available_mcp,
            host_process_surfaces_allowed,
            &mut startup_warnings,
        );
        self.reconcile_connected_mcp_servers(&mut runtime, target_mcp, &sandbox_policy)
            .await?;

        *self.preamble.plugin_instructions.write().unwrap() = plugin_instructions.clone();
        let instructions = build_system_preamble(
            self.workspace_root(),
            &self.managed_surface_reload.primary_profile,
            &plugin_instructions,
            &runtime.tool_visibility_context_snapshot(),
        );
        runtime.replace_base_instructions(instructions);
        *self.memory_backend.write().unwrap() = driver_outcome.primary_memory_backend.clone();
        runtime.replace_user_message_augmentor(driver_outcome.primary_memory_backend.clone().map(
            |backend| {
                Arc::new(
                    crate::backend::memory_recall::WorkspaceMemoryRecallAugmentor::new(backend),
                ) as Arc<dyn UserMessageAugmentor>
            },
        ));

        let side_question_context = Self::side_question_context_from_runtime(&runtime, None);
        let tool_names = runtime.tool_registry_names();
        let startup_diagnostics = crate::backend::boot_mcp::build_startup_diagnostics_snapshot(
            self.workspace_root(),
            &runtime.tool_specs(),
            &self.connected_mcp_servers_snapshot(),
            &plugin_plan,
            &startup_warnings,
            &driver_outcome,
        );
        self.sync_runtime_session_refs(&runtime);
        drop(runtime);

        *self.applied_plugin_surfaces.write().unwrap() = AppliedPluginSurfaceState {
            driver_tool_names: driver_outcome.tool_names.clone(),
        };
        self.store_side_question_context(side_question_context);
        {
            let mut startup = self.startup.write().unwrap();
            startup.tool_names = tool_names;
            startup.startup_diagnostics = startup_diagnostics;
        }
        Ok(())
    }

    async fn reconcile_connected_mcp_servers(
        &self,
        runtime: &mut AgentRuntime,
        target_servers: Vec<McpServerConfig>,
        sandbox_policy: &SandboxPolicy,
    ) -> Result<()> {
        let target_names = target_servers
            .iter()
            .map(|server| server.name.clone())
            .collect::<BTreeSet<_>>();
        let removed_servers = {
            let mut current_servers = self.mcp_servers.write().unwrap();
            let (retained, removed): (Vec<_>, Vec<_>) = current_servers
                .drain(..)
                .partition(|server| target_names.contains(server.server_name.as_str()));
            *current_servers = retained;
            removed
        };
        if !removed_servers.is_empty() {
            let registry = runtime.tool_registry_handle();
            for server in &removed_servers {
                for tool in &server.catalog.tools {
                    registry.remove(tool.name.as_str());
                }
            }
            self.clear_mcp_connection_details_for_names(
                removed_servers
                    .iter()
                    .map(|server| server.server_name.to_string()),
            );
        }

        let connected_names = self
            .mcp_servers
            .read()
            .unwrap()
            .iter()
            .map(|server| server.server_name.clone())
            .collect::<BTreeSet<_>>();
        let pending_configs = target_servers
            .into_iter()
            .filter(|server| !connected_names.contains(&server.name))
            .collect::<Vec<_>>();
        if !pending_configs.is_empty() {
            let outcome = connect_and_prepare_mcp_servers(
                pending_configs
                    .iter()
                    .cloned()
                    .map(|server| {
                        let sandbox_policy = mcp_connection_sandbox_policy(sandbox_policy, &server);
                        (
                            server,
                            McpConnectOptions {
                                process_executor: self.mcp_process_executor.clone(),
                                sandbox_policy,
                                ..Default::default()
                            },
                        )
                    })
                    .collect(),
            )
            .await;
            self.record_mcp_connection_details(&pending_configs, &outcome.details);
            self.attach_connected_stdio_mcp_servers(runtime, outcome.connected);
        } else {
            self.rebuild_mcp_resource_tools(runtime);
        }
        Ok(())
    }
}

fn relativize_to_workspace(workspace_root: &Path, path: &Path) -> String {
    path.strip_prefix(workspace_root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn resolved_plugin_roots(
    workspace_root: &Path,
    plugins: &nanoclaw_config::PluginsConfig,
) -> Vec<PathBuf> {
    let mut roots = plugins
        .roots
        .iter()
        .map(|root| {
            let path = PathBuf::from(root);
            if path.is_absolute() {
                path
            } else {
                workspace_root.join(path)
            }
        })
        .collect::<Vec<_>>();
    if plugins.include_builtin {
        roots.push(workspace_root.join("builtin-plugins"));
    }
    roots.sort();
    roots.dedup();
    roots
}

fn plugin_kind_label(kind: agent::plugins::PluginKind) -> String {
    match kind {
        agent::plugins::PluginKind::Bundle => "bundle".to_string(),
        agent::plugins::PluginKind::Memory => "memory".to_string(),
    }
}

fn plugin_contribution_summary(plugin: &agent::plugins::PluginState) -> String {
    let contributions = &plugin.contributions;
    let mut parts = Vec::new();
    if contributions.instruction_count > 0 {
        parts.push(format!("instructions={}", contributions.instruction_count));
    }
    if !contributions.skill_roots.is_empty() {
        parts.push(format!("skills={}", contributions.skill_roots.len()));
    }
    if contributions.custom_tool_root_count > 0 {
        parts.push(format!(
            "custom_tools={}",
            contributions.custom_tool_root_count
        ));
    }
    if !contributions.hook_names.is_empty() {
        parts.push(format!("hooks={}", contributions.hook_names.len()));
    }
    if !contributions.mcp_servers.is_empty() {
        parts.push(format!("mcp={}", contributions.mcp_servers.len()));
    }
    if let Some(driver) = contributions.runtime_driver.as_ref() {
        parts.push(format!("runtime={driver}"));
    }
    if parts.is_empty() {
        "no declarative or runtime contributions".to_string()
    } else {
        parts.join(", ")
    }
}

fn remove_plugin_tools(registry: &ToolRegistry) {
    let names = registry
        .specs()
        .into_iter()
        .filter_map(|spec| match spec.source {
            agent::types::ToolSource::Plugin { .. } => Some(spec.name.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    remove_named_tools(registry, &names);
}

fn remove_named_tools(registry: &ToolRegistry, names: &[String]) {
    for name in names {
        registry.remove(name);
    }
}

fn filter_runtime_hooks_for_host_surfaces(
    hooks: Vec<HookRegistration>,
    host_process_surfaces_allowed: bool,
    startup_warnings: &mut Vec<String>,
) -> Vec<HookRegistration> {
    if host_process_surfaces_allowed {
        return hooks;
    }

    let (retained, blocked): (Vec<_>, Vec<_>) = hooks
        .into_iter()
        .partition(|hook| !matches!(hook.handler, HookHandler::Command(_)));
    if !blocked.is_empty() {
        startup_warnings.push(format!(
            "{COMMAND_HOOK_DISABLED_WARNING_PREFIX} {}",
            blocked
                .iter()
                .map(|hook| hook.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    retained
}

fn filter_disabled_builtin_skills(
    workspace_root: &Path,
    disabled_builtin_skills: &BTreeSet<String>,
    skill_catalog: SkillCatalog,
) -> SkillCatalog {
    if disabled_builtin_skills.is_empty() {
        return skill_catalog;
    }
    let builtin_root = builtin_skill_root(workspace_root);
    let filtered = skill_catalog
        .all()
        .into_iter()
        .filter(|skill| {
            !(skill.provenance.root.path == builtin_root
                && disabled_builtin_skills.contains(&skill.name))
        })
        .collect();
    SkillCatalog::from_parts(skill_catalog.roots(), filtered)
}

fn filter_mcp_servers_for_host_surfaces(
    servers: Vec<McpServerConfig>,
    host_process_surfaces_allowed: bool,
    startup_warnings: &mut Vec<String>,
) -> Vec<McpServerConfig> {
    if host_process_surfaces_allowed {
        return servers;
    }

    let (retained, blocked): (Vec<_>, Vec<_>) = servers
        .into_iter()
        .partition(|server| !matches!(server.transport, McpTransportConfig::Stdio { .. }));
    if !blocked.is_empty() {
        startup_warnings.push(format!(
            "{STDIO_MCP_DISABLED_WARNING_PREFIX} {}",
            blocked
                .iter()
                .map(|server| server.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    retained
}
