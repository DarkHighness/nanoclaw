use super::super::{RuntimeTui, TuiState, prompt_to_text, resource_to_text};
use crate::TuiCommand;
use agent::mcp::McpPromptArgument;
use serde_json::Value;

impl RuntimeTui {
    pub(in crate::app) async fn apply_mcp_command(
        &mut self,
        command: TuiCommand,
        state: &mut TuiState,
    ) -> anyhow::Result<bool> {
        match command {
            TuiCommand::Mcp => {
                state.sidebar = self
                    .mcp_servers
                    .iter()
                    .map(|server| {
                        format!(
                            "server: {}  tools={} prompts={} resources={}",
                            server.server_name,
                            server.catalog.tools.len(),
                            server.catalog.prompts.len(),
                            server.catalog.resources.len()
                        )
                    })
                    .collect();
                state.sidebar_title = "MCP".to_string();
                state.status = "Listed MCP servers".to_string();
                Ok(false)
            }
            TuiCommand::Prompts => {
                state.sidebar = self
                    .mcp_servers
                    .iter()
                    .flat_map(|server| {
                        server
                            .catalog
                            .prompts
                            .iter()
                            .map(|prompt| {
                                let suffix = prompt_argument_suffix(&prompt.arguments);
                                format!(
                                    "{}:{}{}{}",
                                    server.server_name,
                                    prompt.name,
                                    suffix,
                                    if prompt.description.is_empty() {
                                        String::new()
                                    } else {
                                        format!(" - {}", prompt.description)
                                    }
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect();
                state.sidebar_title = "Prompts".to_string();
                state.status = "Listed MCP prompts".to_string();
                Ok(false)
            }
            TuiCommand::Resources => {
                state.sidebar = self
                    .mcp_servers
                    .iter()
                    .flat_map(|server| {
                        server
                            .catalog
                            .resources
                            .iter()
                            .map(|resource| {
                                format!(
                                    "{}:{}{}",
                                    server.server_name,
                                    resource.uri,
                                    resource
                                        .mime_type
                                        .as_deref()
                                        .map(|mime| format!(" [{mime}]"))
                                        .unwrap_or_default()
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect();
                state.sidebar_title = "Resources".to_string();
                state.status = "Listed MCP resources".to_string();
                Ok(false)
            }
            TuiCommand::Prompt {
                server_name,
                prompt_name,
            } => {
                let server = self
                    .mcp_servers
                    .iter()
                    .find(|server| server.server_name == server_name)
                    .ok_or_else(|| anyhow::anyhow!("unknown MCP server: {server_name}"))?;
                let prompt = server.client.get_prompt(&prompt_name, Value::Null).await?;
                state.input = prompt_to_text(&prompt);
                state.sidebar = vec![
                    format!("prompt: {server_name}/{prompt_name}"),
                    format!("arguments: {}", prompt_argument_names(&prompt.arguments)),
                ];
                state.sidebar_title = "Prompt".to_string();
                state.status = format!("Loaded MCP prompt {server_name}/{prompt_name} into input");
                Ok(false)
            }
            TuiCommand::Resource { server_name, uri } => {
                let server = self
                    .mcp_servers
                    .iter()
                    .find(|server| server.server_name == server_name)
                    .ok_or_else(|| anyhow::anyhow!("unknown MCP server: {server_name}"))?;
                let resource = server.client.read_resource(&uri).await?;
                state.input = resource_to_text(&resource);
                state.sidebar = vec![
                    format!("resource: {server_name}:{}", resource.uri),
                    format!(
                        "mime: {}",
                        resource
                            .mime_type
                            .clone()
                            .unwrap_or_else(|| "unknown".to_string())
                    ),
                ];
                state.sidebar_title = "Resource".to_string();
                state.status = format!(
                    "Loaded MCP resource {server_name}:{} into input",
                    resource.uri
                );
                Ok(false)
            }
            _ => unreachable!("mcp handler received non-mcp command"),
        }
    }
}

fn prompt_argument_suffix(arguments: &[McpPromptArgument]) -> String {
    let names = prompt_argument_names(arguments);
    if names == "none" {
        String::new()
    } else {
        format!(" ({names})")
    }
}

fn prompt_argument_names(arguments: &[McpPromptArgument]) -> String {
    if arguments.is_empty() {
        "none".to_string()
    } else {
        arguments
            .iter()
            .map(|argument| {
                if argument.required {
                    format!("{}*", argument.name)
                } else {
                    argument.name.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}
