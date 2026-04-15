use super::*;

impl CodeAgentTui {
    pub(crate) async fn load_mcp_prompt_into_input(
        &mut self,
        server_name: String,
        prompt_name: String,
    ) -> Result<()> {
        let loaded: LoadedMcpPrompt = self
            .run_ui(UIAsyncCommand::LoadMcpPrompt {
                server_name: server_name.clone(),
                prompt_name: prompt_name.clone(),
            })
            .await?;
        self.ui_state.mutate(move |state| {
            let inspector = build_mcp_prompt_inspector(&loaded);
            state.restore_input_draft(state::composer_draft_from_messages(&loaded.input_messages));
            state.show_main_view("Prompt", inspector);
            state.status = format!("Loaded MCP prompt {server_name}/{prompt_name} into input");
            state.push_activity(format!("loaded mcp prompt {server_name}/{prompt_name}"));
        });
        Ok(())
    }

    pub(crate) async fn load_mcp_resource_into_input(
        &mut self,
        server_name: String,
        uri: String,
    ) -> Result<()> {
        let loaded: LoadedMcpResource = self
            .run_ui(UIAsyncCommand::LoadMcpResource {
                server_name: server_name.clone(),
                uri: uri.clone(),
            })
            .await?;
        self.ui_state.mutate(move |state| {
            let inspector = build_mcp_resource_inspector(&loaded);
            state.restore_input_draft(state::composer_draft_from_parts(&loaded.input_parts));
            state.show_main_view("Resource", inspector);
            state.status = format!("Loaded MCP resource {server_name}:{uri} into input");
            state.push_activity(format!("loaded mcp resource {server_name}:{uri}"));
        });
        Ok(())
    }

    pub(crate) async fn apply_mcp_command(&mut self, command: SlashCommand) -> Result<bool> {
        match command {
            SlashCommand::Mcp => {
                self.open_managed_toggle_picker(state::ManagedTogglePickerKind::Mcp)
                    .await?;
                Ok(false)
            }
            SlashCommand::Skill => {
                self.open_managed_toggle_picker(state::ManagedTogglePickerKind::Skill)
                    .await?;
                Ok(false)
            }
            SlashCommand::Plugin => {
                self.open_managed_toggle_picker(state::ManagedTogglePickerKind::Plugin)
                    .await?;
                Ok(false)
            }
            SlashCommand::Prompts => {
                let prompts: Vec<McpPromptSummary> =
                    self.run_ui(UIAsyncCommand::ListMcpPrompts).await?;
                self.ui_state.mutate(move |state| {
                    let lines = if prompts.is_empty() {
                        vec![
                            InspectorEntry::section("MCP Prompts"),
                            InspectorEntry::Muted("No MCP prompts available.".to_string()),
                        ]
                    } else {
                        std::iter::once(InspectorEntry::section("MCP Prompts"))
                            .chain(prompts.iter().map(format_mcp_prompt_summary_line))
                            .collect()
                    };
                    state.show_main_view("Prompts", lines);
                    state.status = "Listed MCP prompts".to_string();
                    state.push_activity("listed mcp prompts");
                });
                Ok(false)
            }
            SlashCommand::Resources => {
                let resources: Vec<McpResourceSummary> =
                    self.run_ui(UIAsyncCommand::ListMcpResources).await?;
                self.ui_state.mutate(move |state| {
                    let lines = if resources.is_empty() {
                        vec![
                            InspectorEntry::section("MCP Resources"),
                            InspectorEntry::Muted("No MCP resources available.".to_string()),
                        ]
                    } else {
                        std::iter::once(InspectorEntry::section("MCP Resources"))
                            .chain(resources.iter().map(format_mcp_resource_summary_line))
                            .collect()
                    };
                    state.show_main_view("Resources", lines);
                    state.status = "Listed MCP resources".to_string();
                    state.push_activity("listed mcp resources");
                });
                Ok(false)
            }
            _ => unreachable!("mcp handler received non-mcp command"),
        }
    }

    pub(crate) async fn load_managed_toggle_items(
        &self,
        kind: state::ManagedTogglePickerKind,
    ) -> Result<Vec<state::ManagedTogglePickerItem>> {
        match kind {
            state::ManagedTogglePickerKind::Mcp => {
                let servers: Vec<ManagedMcpServerSummary> =
                    self.run_ui(UIAsyncCommand::ListManagedMcpServers).await?;
                Ok(servers
                    .into_iter()
                    .map(|server| {
                        let detail = format_managed_mcp_server_detail(&server);
                        state::ManagedTogglePickerItem {
                            id: server.name.clone(),
                            label: server.name,
                            detail,
                            enabled: server.enabled,
                        }
                    })
                    .collect())
            }
            state::ManagedTogglePickerKind::Skill => {
                let skills: Vec<ManagedSkillSummary> =
                    self.run_ui(UIAsyncCommand::ListManagedSkills).await?;
                Ok(skills
                    .into_iter()
                    .map(|skill| {
                        let detail = format_managed_skill_detail(&skill);
                        state::ManagedTogglePickerItem {
                            id: skill.name.clone(),
                            label: skill.name,
                            detail,
                            enabled: skill.enabled,
                        }
                    })
                    .collect())
            }
            state::ManagedTogglePickerKind::Plugin => {
                let plugins: Vec<ManagedPluginSummary> =
                    self.run_ui(UIAsyncCommand::ListManagedPlugins).await?;
                Ok(plugins
                    .into_iter()
                    .map(|plugin| {
                        let detail = format_managed_plugin_detail(&plugin);
                        state::ManagedTogglePickerItem {
                            id: plugin.plugin_id.clone(),
                            label: plugin.plugin_id,
                            detail,
                            enabled: plugin.enabled,
                        }
                    })
                    .collect())
            }
        }
    }

    pub(crate) async fn open_managed_toggle_picker(
        &mut self,
        kind: state::ManagedTogglePickerKind,
    ) -> Result<()> {
        let title = managed_toggle_picker_title(kind);
        let status = open_managed_toggle_picker_status(kind);
        let activity = open_managed_toggle_picker_activity(kind);
        let items = self.load_managed_toggle_items(kind).await?;
        self.ui_state.mutate(move |state| {
            state.open_managed_toggle_picker(kind, title, items);
            state.status = status.to_string();
            state.push_activity(activity);
        });
        Ok(())
    }
}

fn format_managed_mcp_server_detail(summary: &ManagedMcpServerSummary) -> String {
    let connection = if summary.connected {
        "connected"
    } else {
        "disconnected"
    };
    format!(
        "{} · {} · tools={} · prompts={} · resources={}",
        summary.transport,
        connection,
        summary.tool_count,
        summary.prompt_count,
        summary.resource_count
    )
}

fn format_managed_skill_detail(summary: &ManagedSkillSummary) -> String {
    let source = if summary.builtin {
        "built-in"
    } else {
        "managed"
    };
    if summary.description.trim().is_empty() {
        format!("{source} · {}", summary.path)
    } else {
        format!("{source} · {} · {}", summary.path, summary.description)
    }
}

fn format_managed_plugin_detail(summary: &ManagedPluginSummary) -> String {
    let path = if summary.path.trim().is_empty() {
        "path unavailable".to_string()
    } else {
        summary.path.clone()
    };
    format!(
        "{} · {} · {}",
        summary.kind, path, summary.contribution_summary
    )
}

pub(crate) fn managed_toggle_picker_title(kind: state::ManagedTogglePickerKind) -> &'static str {
    match kind {
        state::ManagedTogglePickerKind::Mcp => "MCP",
        state::ManagedTogglePickerKind::Skill => "Skills",
        state::ManagedTogglePickerKind::Plugin => "Plugins",
    }
}

pub(crate) fn managed_toggle_picker_subject(kind: state::ManagedTogglePickerKind) -> &'static str {
    match kind {
        state::ManagedTogglePickerKind::Mcp => "MCP server",
        state::ManagedTogglePickerKind::Skill => "skill",
        state::ManagedTogglePickerKind::Plugin => "plugin",
    }
}

fn open_managed_toggle_picker_status(kind: state::ManagedTogglePickerKind) -> &'static str {
    match kind {
        state::ManagedTogglePickerKind::Mcp => "Opened MCP manager",
        state::ManagedTogglePickerKind::Skill => "Opened skill manager",
        state::ManagedTogglePickerKind::Plugin => "Opened plugin manager",
    }
}

fn open_managed_toggle_picker_activity(kind: state::ManagedTogglePickerKind) -> &'static str {
    match kind {
        state::ManagedTogglePickerKind::Mcp => "opened mcp manager",
        state::ManagedTogglePickerKind::Skill => "opened skill manager",
        state::ManagedTogglePickerKind::Plugin => "opened plugin manager",
    }
}
