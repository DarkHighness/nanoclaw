use crate::{
    ConnectedMcpServer, McpCatalog, McpPrompt, McpPromptArgument, McpResource, McpServerConfig,
    McpTransportConfig,
};
use agent_core_types::{
    Message, MessagePart, MessageRole, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult,
    ToolSpec, new_opaque_id,
};
use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use http::{HeaderName, HeaderValue};
use rmcp::ServiceExt;
use rmcp::model::{
    CallToolRequestParams, Content, GetPromptRequestParams, PromptMessage, PromptMessageContent,
    PromptMessageRole, RawContent, ReadResourceRequestParams, ResourceContents, Tool,
};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::Mutex;

#[async_trait]
pub trait McpClient: Send + Sync {
    async fn catalog(&self) -> Result<McpCatalog>;
    async fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<ToolResult>;
    async fn read_resource(&self, uri: &str) -> Result<McpResource>;
    async fn get_prompt(&self, name: &str, arguments: Value) -> Result<McpPrompt>;
}

pub async fn connect_mcp_server(config: &McpServerConfig) -> Result<Arc<dyn McpClient>> {
    Ok(Arc::new(RmcpClient::connect(config).await?))
}

pub async fn connect_mcp_servers(configs: &[McpServerConfig]) -> Result<Vec<Arc<dyn McpClient>>> {
    let mut clients = Vec::new();
    for config in configs {
        clients.push(connect_mcp_server(config).await?);
    }
    Ok(clients)
}

pub async fn connect_and_catalog_mcp_servers(
    configs: &[McpServerConfig],
) -> Result<Vec<ConnectedMcpServer>> {
    let mut servers = Vec::new();
    for config in configs {
        let client = connect_mcp_server(config).await?;
        let catalog = client.catalog().await?;
        servers.push(ConnectedMcpServer {
            server_name: config.name.clone(),
            client,
            catalog,
        });
    }
    Ok(servers)
}

pub struct RmcpClient {
    server_name: String,
    peer: rmcp::Peer<rmcp::RoleClient>,
    _service: Mutex<rmcp::service::RunningService<rmcp::RoleClient, ()>>,
}

impl RmcpClient {
    async fn connect(config: &McpServerConfig) -> Result<Self> {
        let service = match &config.transport {
            McpTransportConfig::Stdio {
                command,
                args,
                env,
                cwd,
            } => connect_stdio_transport(command, args, env, cwd.as_deref()).await?,
            McpTransportConfig::StreamableHttp { url, headers } => {
                connect_streamable_http_transport(url, headers).await?
            }
        };

        let peer = service.peer().clone();
        Ok(Self {
            server_name: config.name.clone(),
            peer,
            _service: Mutex::new(service),
        })
    }
}

#[async_trait]
impl McpClient for RmcpClient {
    async fn catalog(&self) -> Result<McpCatalog> {
        let tools = self
            .peer
            .list_all_tools()
            .await?
            .into_iter()
            .map(|tool| tool_spec_from_rmcp(&self.server_name, tool))
            .collect::<Result<Vec<_>>>()?;
        let prompts = self
            .peer
            .list_all_prompts()
            .await?
            .into_iter()
            .map(mcp_prompt_from_listing)
            .collect();
        let resources = self
            .peer
            .list_all_resources()
            .await?
            .into_iter()
            .map(mcp_resource_from_listing)
            .collect();

        Ok(McpCatalog {
            server_name: self.server_name.clone(),
            tools,
            prompts,
            resources,
        })
    }

    async fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<ToolResult> {
        let mut params = CallToolRequestParams::new(tool_name.to_string());
        if !arguments.is_null() {
            let Value::Object(map) = arguments else {
                bail!("MCP tool arguments must serialize as a JSON object");
            };
            params = params.with_arguments(map);
        }

        let result = self.peer.call_tool(params).await?;
        Ok(tool_result_from_rmcp(tool_name, result))
    }

    async fn read_resource(&self, uri: &str) -> Result<McpResource> {
        let result = self
            .peer
            .read_resource(ReadResourceRequestParams::new(uri))
            .await?;
        Ok(mcp_resource_from_contents(uri, result.contents))
    }

    async fn get_prompt(&self, name: &str, arguments: Value) -> Result<McpPrompt> {
        let mut params = GetPromptRequestParams::new(name);
        if !arguments.is_null() {
            let Value::Object(map) = arguments else {
                bail!("MCP prompt arguments must serialize as a JSON object");
            };
            params = params.with_arguments(map);
        }
        let result = self.peer.get_prompt(params).await?;
        Ok(McpPrompt {
            name: name.to_string(),
            title: None,
            description: result.description.unwrap_or_default(),
            arguments: Vec::new(),
            messages: prompt_messages_to_messages(&result.messages),
        })
    }
}

#[derive(Clone)]
pub struct MockMcpClient {
    catalog: McpCatalog,
    tool_handler: Arc<dyn Fn(&str, Value) -> Result<ToolResult> + Send + Sync>,
}

impl MockMcpClient {
    #[must_use]
    pub fn new(
        catalog: McpCatalog,
        tool_handler: Arc<dyn Fn(&str, Value) -> Result<ToolResult> + Send + Sync>,
    ) -> Self {
        Self {
            catalog,
            tool_handler,
        }
    }
}

#[async_trait]
impl McpClient for MockMcpClient {
    async fn catalog(&self) -> Result<McpCatalog> {
        Ok(self.catalog.clone())
    }

    async fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<ToolResult> {
        (self.tool_handler)(tool_name, arguments)
    }

    async fn read_resource(&self, uri: &str) -> Result<McpResource> {
        self.catalog
            .resources
            .iter()
            .find(|resource| resource.uri == uri)
            .cloned()
            .ok_or_else(|| anyhow!("resource not found: {uri}"))
    }

    async fn get_prompt(&self, name: &str, _arguments: Value) -> Result<McpPrompt> {
        self.catalog
            .prompts
            .iter()
            .find(|prompt| prompt.name == name)
            .cloned()
            .ok_or_else(|| anyhow!("prompt not found: {name}"))
    }
}

async fn connect_stdio_transport(
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    cwd: Option<&str>,
) -> Result<rmcp::service::RunningService<rmcp::RoleClient, ()>> {
    let mut process = Command::new(command);
    process.args(args);
    process.envs(env);
    if let Some(cwd) = cwd {
        process.current_dir(cwd);
    }
    let transport = TokioChildProcess::new(process)?;
    Ok(().serve(transport).await?)
}

async fn connect_streamable_http_transport(
    url: &str,
    headers: &BTreeMap<String, String>,
) -> Result<rmcp::service::RunningService<rmcp::RoleClient, ()>> {
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(url.to_string())
            .custom_headers(http_headers(headers)?),
    );
    Ok(().serve(transport).await?)
}

fn http_headers(headers: &BTreeMap<String, String>) -> Result<HashMap<HeaderName, HeaderValue>> {
    headers
        .iter()
        .map(|(name, value)| {
            Ok((
                HeaderName::from_bytes(name.as_bytes())?,
                HeaderValue::from_str(value)?,
            ))
        })
        .collect()
}

fn tool_spec_from_rmcp(server_name: &str, tool: Tool) -> Result<ToolSpec> {
    let mut annotations = BTreeMap::new();
    if let Some(title) = tool.title {
        annotations.insert("title".to_string(), Value::String(title));
    }
    if let Some(meta) = tool.meta {
        annotations.insert("meta".to_string(), serde_json::to_value(meta)?);
    }
    if let Some(output_schema) = tool.output_schema {
        annotations.insert(
            "output_schema".to_string(),
            Value::Object((*output_schema).clone()),
        );
    }
    if let Some(tool_annotations) = tool.annotations {
        if let Value::Object(values) = serde_json::to_value(tool_annotations)? {
            annotations.extend(values);
        }
    }
    if let Some(execution) = tool.execution {
        annotations.insert(
            "mcp_execution".to_string(),
            serde_json::to_value(execution)?,
        );
    }

    Ok(ToolSpec {
        name: tool.name.to_string(),
        description: tool
            .description
            .map(|value| value.to_string())
            .unwrap_or_default(),
        input_schema: Value::Object((*tool.input_schema).clone()),
        output_mode: ToolOutputMode::ContentParts,
        origin: ToolOrigin::Mcp {
            server_name: server_name.to_string(),
        },
        annotations,
    })
}

fn tool_result_from_rmcp(tool_name: &str, result: rmcp::model::CallToolResult) -> ToolResult {
    let metadata = result
        .structured_content
        .clone()
        .or_else(|| serde_json::to_value(result.meta).ok());

    ToolResult {
        id: ToolCallId::new(),
        call_id: new_opaque_id(),
        tool_name: tool_name.to_string(),
        parts: rmcp_content_to_parts(result.content),
        metadata,
        is_error: result.is_error.unwrap_or(false),
    }
}

fn rmcp_content_to_parts(content: Vec<Content>) -> Vec<MessagePart> {
    let mut parts = Vec::new();
    for item in content {
        match item.raw {
            RawContent::Text(text) => parts.push(MessagePart::text(text.text)),
            RawContent::Image(image) => parts.push(MessagePart::Image {
                mime_type: image.mime_type,
                data_base64: image.data,
            }),
            RawContent::Resource(resource) => match resource.resource {
                ResourceContents::TextResourceContents {
                    uri,
                    mime_type,
                    text,
                    ..
                } => parts.push(MessagePart::Resource {
                    uri,
                    mime_type,
                    text: Some(text),
                    metadata: None,
                }),
                ResourceContents::BlobResourceContents {
                    uri,
                    mime_type,
                    blob,
                    ..
                } => parts.push(MessagePart::Resource {
                    uri,
                    mime_type,
                    text: Some(blob),
                    metadata: None,
                }),
            },
            RawContent::ResourceLink(resource) => parts.push(MessagePart::Resource {
                uri: resource.uri,
                mime_type: resource.mime_type,
                text: resource.description,
                metadata: None,
            }),
            RawContent::Audio(audio) => parts.push(MessagePart::File {
                file_name: None,
                mime_type: Some(audio.mime_type),
                data_base64: Some(audio.data),
                uri: None,
            }),
        }
    }

    if parts.is_empty() {
        parts.push(MessagePart::text(String::new()));
    }
    parts
}

fn mcp_prompt_from_listing(prompt: rmcp::model::Prompt) -> McpPrompt {
    McpPrompt {
        name: prompt.name,
        title: prompt.title,
        description: prompt.description.unwrap_or_default(),
        arguments: prompt
            .arguments
            .unwrap_or_default()
            .into_iter()
            .map(|argument| McpPromptArgument {
                name: argument.name,
                title: argument.title,
                description: argument.description,
                required: argument.required.unwrap_or(false),
            })
            .collect(),
        messages: Vec::new(),
    }
}

fn mcp_resource_from_listing(resource: rmcp::model::Resource) -> McpResource {
    McpResource {
        uri: resource.uri.clone(),
        name: resource.name.clone(),
        title: resource.title.clone(),
        description: resource.description.clone().unwrap_or_default(),
        mime_type: resource.mime_type.clone(),
        parts: Vec::new(),
    }
}

fn mcp_resource_from_contents(uri: &str, contents: Vec<ResourceContents>) -> McpResource {
    let first_text = contents.iter().find_map(|content| match content {
        ResourceContents::TextResourceContents { mime_type, .. } => mime_type.clone(),
        ResourceContents::BlobResourceContents { mime_type, .. } => mime_type.clone(),
    });
    McpResource {
        uri: uri.to_string(),
        name: uri.rsplit('/').next().unwrap_or(uri).to_string(),
        title: None,
        description: "MCP resource".to_string(),
        mime_type: first_text,
        parts: resource_contents_to_parts(contents),
    }
}

fn resource_contents_to_parts(contents: Vec<ResourceContents>) -> Vec<MessagePart> {
    let parts = contents
        .into_iter()
        .map(|content| match content {
            ResourceContents::TextResourceContents {
                uri,
                mime_type,
                text,
                meta,
            } => MessagePart::Resource {
                uri,
                mime_type,
                text: Some(text),
                metadata: meta.and_then(|value| serde_json::to_value(value).ok()),
            },
            ResourceContents::BlobResourceContents {
                uri,
                mime_type,
                blob,
                ..
            } => MessagePart::File {
                file_name: uri.rsplit('/').next().map(ToString::to_string),
                mime_type,
                data_base64: Some(blob),
                uri: Some(uri),
            },
        })
        .collect::<Vec<_>>();

    if parts.is_empty() {
        vec![MessagePart::text(String::new())]
    } else {
        parts
    }
}

fn prompt_messages_to_messages(messages: &[PromptMessage]) -> Vec<Message> {
    messages.iter().map(prompt_message_to_message).collect()
}

fn prompt_message_to_message(message: &PromptMessage) -> Message {
    let role = match message.role {
        PromptMessageRole::User => MessageRole::User,
        PromptMessageRole::Assistant => MessageRole::Assistant,
    };
    let part = match &message.content {
        PromptMessageContent::Text { text } => MessagePart::text(text.clone()),
        PromptMessageContent::Image { image } => MessagePart::Image {
            mime_type: image.mime_type.clone(),
            data_base64: image.data.clone(),
        },
        PromptMessageContent::Resource { resource } => match &resource.resource {
            ResourceContents::TextResourceContents {
                uri,
                mime_type,
                text,
                meta,
            } => MessagePart::Resource {
                uri: uri.clone(),
                mime_type: mime_type.clone(),
                text: Some(text.clone()),
                metadata: meta
                    .clone()
                    .and_then(|value| serde_json::to_value(value).ok()),
            },
            ResourceContents::BlobResourceContents {
                uri,
                mime_type,
                blob,
                ..
            } => MessagePart::File {
                file_name: uri.rsplit('/').next().map(ToString::to_string),
                mime_type: mime_type.clone(),
                data_base64: Some(blob.clone()),
                uri: Some(uri.clone()),
            },
        },
        PromptMessageContent::ResourceLink { link } => MessagePart::Resource {
            uri: link.uri.clone(),
            mime_type: link.mime_type.clone(),
            text: link.description.clone(),
            metadata: link
                .meta
                .clone()
                .and_then(|value| serde_json::to_value(value).ok()),
        },
    };
    Message::new(role, vec![part])
}
