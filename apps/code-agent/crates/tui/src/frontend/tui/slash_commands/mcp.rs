use super::*;

impl CodeAgentTui {
    pub(crate) async fn apply_mcp_command(&mut self, command: SlashCommand) -> Result<bool> {
        match command {
            SlashCommand::Mcp => {
                let servers: Vec<McpServerSummary> =
                    self.run_ui(UIAsyncCommand::ListMcpServers).await?;
                self.ui_state.mutate(move |state| {
                    let lines = if servers.is_empty() {
                        vec![
                            InspectorEntry::section("MCP"),
                            InspectorEntry::Muted("No MCP servers connected.".to_string()),
                        ]
                    } else {
                        std::iter::once(InspectorEntry::section("MCP"))
                            .chain(servers.iter().map(format_mcp_server_summary_line))
                            .collect()
                    };
                    state.show_main_view("MCP", lines);
                    state.status = "Listed MCP servers".to_string();
                    state.push_activity("listed mcp servers");
                });
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
            SlashCommand::Prompt {
                server_name,
                prompt_name,
            } => {
                let loaded: LoadedMcpPrompt = self
                    .run_ui(UIAsyncCommand::LoadMcpPrompt {
                        server_name: server_name.clone(),
                        prompt_name: prompt_name.clone(),
                    })
                    .await?;
                self.ui_state.mutate(move |state| {
                    let inspector = build_mcp_prompt_inspector(&loaded);
                    state.restore_input_draft(state::composer_draft_from_messages(
                        &loaded.input_messages,
                    ));
                    state.show_main_view("Prompt", inspector);
                    state.status =
                        format!("Loaded MCP prompt {server_name}/{prompt_name} into input");
                    state.push_activity(format!("loaded mcp prompt {server_name}/{prompt_name}"));
                });
                Ok(false)
            }
            SlashCommand::Resource { server_name, uri } => {
                let loaded: LoadedMcpResource = self
                    .run_ui(UIAsyncCommand::LoadMcpResource {
                        server_name: server_name.clone(),
                        uri: uri.clone(),
                    })
                    .await?;
                self.ui_state.mutate(move |state| {
                    let inspector = build_mcp_resource_inspector(&loaded);
                    state
                        .restore_input_draft(state::composer_draft_from_parts(&loaded.input_parts));
                    state.show_main_view("Resource", inspector);
                    state.status = format!("Loaded MCP resource {server_name}:{uri} into input");
                    state.push_activity(format!("loaded mcp resource {server_name}:{uri}"));
                });
                Ok(false)
            }
            _ => unreachable!("mcp handler received non-mcp command"),
        }
    }
}
