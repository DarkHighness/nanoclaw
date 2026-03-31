use crate::{
    ConnectedMcpServer, McpCatalog, McpError, McpPrompt, McpPromptArgument, McpResource,
    McpResourceTemplate, McpServerConfig, McpTransportConfig, Result,
};
use async_trait::async_trait;
use futures::{StreamExt, TryStreamExt, stream};
use http::{HeaderName, HeaderValue};
use rmcp::ServiceExt;
use rmcp::model::{
    CallToolRequestParams, Content, GetPromptRequestParams, PromptMessage, PromptMessageContent,
    PromptMessageRole, RawContent, ReadResourceRequestParams, ResourceContents, Tool,
};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use sandbox::{
    ExecRequest, ExecutionOrigin, HostProcessExecutor, ProcessExecutor, ProcessStdio, SandboxPolicy,
};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use tracing::{debug, info};
use types::{
    McpServerName, Message, MessagePart, MessageRole, ToolApprovalProfile, ToolCallId, ToolKind,
    ToolOrigin, ToolOutputMode, ToolResult, ToolSource, ToolSpec, new_opaque_id,
};

const MCP_CONNECT_CONCURRENCY_LIMIT: usize = 8;

#[async_trait]
pub trait McpClient: Send + Sync {
    async fn catalog(&self) -> Result<McpCatalog>;
    async fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<ToolResult>;
    async fn read_resource(&self, uri: &str) -> Result<McpResource>;
    async fn get_prompt(&self, name: &str, arguments: Value) -> Result<McpPrompt>;
}

#[derive(Clone)]
pub struct McpConnectOptions {
    pub process_executor: Arc<dyn ProcessExecutor>,
    pub sandbox_policy: SandboxPolicy,
}

impl Default for McpConnectOptions {
    fn default() -> Self {
        Self {
            process_executor: Arc::new(HostProcessExecutor),
            sandbox_policy: SandboxPolicy::default(),
        }
    }
}

pub async fn connect_mcp_server(config: &McpServerConfig) -> Result<Arc<dyn McpClient>> {
    connect_mcp_server_with_options(config, McpConnectOptions::default()).await
}

pub async fn connect_mcp_servers(configs: &[McpServerConfig]) -> Result<Vec<Arc<dyn McpClient>>> {
    connect_mcp_servers_with_options(configs, McpConnectOptions::default()).await
}

pub async fn connect_mcp_server_with_options(
    config: &McpServerConfig,
    options: McpConnectOptions,
) -> Result<Arc<dyn McpClient>> {
    Ok(Arc::new(RmcpClient::connect(config, &options).await?))
}

pub async fn connect_mcp_servers_with_options(
    configs: &[McpServerConfig],
    options: McpConnectOptions,
) -> Result<Vec<Arc<dyn McpClient>>> {
    info!(server_count = configs.len(), "connecting MCP servers");
    let tasks = configs
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, config)| {
            let options = options.clone();
            async move {
                let client = connect_mcp_server_with_options(&config, options).await?;
                Ok::<_, McpError>((index, client))
            }
        })
        .collect::<Vec<_>>();
    run_indexed_tasks_ordered(tasks, MCP_CONNECT_CONCURRENCY_LIMIT).await
}

pub async fn connect_and_catalog_mcp_servers(
    configs: &[McpServerConfig],
) -> Result<Vec<ConnectedMcpServer>> {
    connect_and_catalog_mcp_servers_with_options(configs, McpConnectOptions::default()).await
}

pub async fn connect_and_catalog_mcp_servers_with_options(
    configs: &[McpServerConfig],
    options: McpConnectOptions,
) -> Result<Vec<ConnectedMcpServer>> {
    info!(
        server_count = configs.len(),
        "connecting and cataloging MCP servers"
    );
    let tasks = configs
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, config)| {
            let options = options.clone();
            async move {
                let client = connect_mcp_server_with_options(&config, options).await?;
                let catalog = client.catalog().await?;
                Ok::<_, McpError>((
                    index,
                    ConnectedMcpServer {
                        server_name: config.name,
                        client,
                        catalog,
                    },
                ))
            }
        })
        .collect::<Vec<_>>();
    run_indexed_tasks_ordered(tasks, MCP_CONNECT_CONCURRENCY_LIMIT).await
}

async fn run_indexed_tasks_ordered<T, E, Fut>(
    tasks: Vec<Fut>,
    concurrency_limit: usize,
) -> std::result::Result<Vec<T>, E>
where
    Fut: std::future::Future<Output = std::result::Result<(usize, T), E>>,
{
    // Connections and remote catalogs can block on network/child-process startup.
    // We bound parallelism to avoid startup stampedes while still eliminating
    // obvious N-by-N serial waits.
    let mut indexed = stream::iter(tasks)
        .buffer_unordered(concurrency_limit.max(1))
        .try_collect::<Vec<_>>()
        .await?;

    // Callers expect outputs to align with the original config order even though
    // each task completes out of order under bounded parallelism.
    indexed.sort_by_key(|(index, _)| *index);
    Ok(indexed.into_iter().map(|(_, value)| value).collect())
}

pub struct RmcpClient {
    server_name: McpServerName,
    peer: rmcp::Peer<rmcp::RoleClient>,
    // The running RMCP service is retained only to keep the transport task
    // alive for the peer. Nothing in the current substrate mutates it after
    // connect, so a synchronous mutex is sufficient and avoids async-lock
    // overhead on a non-awaiting code path.
    _service: Mutex<rmcp::service::RunningService<rmcp::RoleClient, ()>>,
}

impl RmcpClient {
    async fn connect(config: &McpServerConfig, options: &McpConnectOptions) -> Result<Self> {
        debug!(
            server = %config.name,
            transport = match &config.transport {
                McpTransportConfig::Stdio { .. } => "stdio",
                McpTransportConfig::StreamableHttp { .. } => "streamable_http",
            },
            "connecting MCP server"
        );
        let service = match &config.transport {
            McpTransportConfig::Stdio {
                command,
                args,
                env,
                cwd,
            } => {
                connect_stdio_transport(&config.name, command, args, env, cwd.as_deref(), options)
                    .await?
            }
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
            .await
            .map_err(|error| McpError::protocol(error.to_string()))?
            .into_iter()
            .map(|tool| tool_spec_from_rmcp(&self.server_name, tool))
            .collect::<Result<Vec<_>>>()?;
        let prompts = self
            .peer
            .list_all_prompts()
            .await
            .map_err(|error| McpError::protocol(error.to_string()))?
            .into_iter()
            .map(mcp_prompt_from_listing)
            .collect();
        let resources = self
            .peer
            .list_all_resources()
            .await
            .map_err(|error| McpError::protocol(error.to_string()))?
            .into_iter()
            .map(mcp_resource_from_listing)
            .collect();
        let resource_templates = self
            .peer
            .list_all_resource_templates()
            .await
            .map_err(|error| McpError::protocol(error.to_string()))?
            .into_iter()
            .map(mcp_resource_template_from_listing)
            .collect();

        Ok(McpCatalog {
            server_name: self.server_name.clone(),
            tools,
            prompts,
            resources,
            resource_templates,
        })
    }

    async fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<ToolResult> {
        let mut params = CallToolRequestParams::new(tool_name.to_string());
        if !arguments.is_null() {
            let Value::Object(map) = arguments else {
                return Err(McpError::protocol(
                    "MCP tool arguments must serialize as a JSON object",
                ));
            };
            params = params.with_arguments(map);
        }

        let result = self
            .peer
            .call_tool(params)
            .await
            .map_err(|error| McpError::protocol(error.to_string()))?;
        Ok(tool_result_from_rmcp(tool_name, result))
    }

    async fn read_resource(&self, uri: &str) -> Result<McpResource> {
        let result = self
            .peer
            .read_resource(ReadResourceRequestParams::new(uri))
            .await
            .map_err(|error| McpError::protocol(error.to_string()))?;
        Ok(mcp_resource_from_contents(uri, result.contents))
    }

    async fn get_prompt(&self, name: &str, arguments: Value) -> Result<McpPrompt> {
        let mut params = GetPromptRequestParams::new(name);
        if !arguments.is_null() {
            let Value::Object(map) = arguments else {
                return Err(McpError::protocol(
                    "MCP prompt arguments must serialize as a JSON object",
                ));
            };
            params = params.with_arguments(map);
        }
        let result = self
            .peer
            .get_prompt(params)
            .await
            .map_err(|error| McpError::protocol(error.to_string()))?;
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
            .ok_or_else(|| McpError::protocol(format!("resource not found: {uri}")))
    }

    async fn get_prompt(&self, name: &str, _arguments: Value) -> Result<McpPrompt> {
        self.catalog
            .prompts
            .iter()
            .find(|prompt| prompt.name == name)
            .cloned()
            .ok_or_else(|| McpError::protocol(format!("prompt not found: {name}")))
    }
}

async fn connect_stdio_transport(
    server_name: &McpServerName,
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    cwd: Option<&str>,
    options: &McpConnectOptions,
) -> Result<rmcp::service::RunningService<rmcp::RoleClient, ()>> {
    let process = options
        .process_executor
        .prepare(ExecRequest {
            program: command.to_string(),
            args: args.to_vec(),
            cwd: cwd.map(Into::into),
            env: env.clone(),
            stdin: ProcessStdio::Piped,
            stdout: ProcessStdio::Piped,
            stderr: ProcessStdio::Inherit,
            kill_on_drop: true,
            origin: ExecutionOrigin::McpStdioServer {
                server_name: server_name.clone(),
            },
            runtime_scope: Default::default(),
            sandbox_policy: options.sandbox_policy.clone(),
        })
        .map_err(|error| McpError::transport(error.to_string()))?;
    let transport =
        TokioChildProcess::new(process).map_err(|error| McpError::transport(error.to_string()))?;
    ().serve(transport)
        .await
        .map_err(|error| McpError::transport(error.to_string()))
}

async fn connect_streamable_http_transport(
    url: &str,
    headers: &BTreeMap<String, String>,
) -> Result<rmcp::service::RunningService<rmcp::RoleClient, ()>> {
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(url.to_string())
            .custom_headers(http_headers(headers)?),
    );
    ().serve(transport)
        .await
        .map_err(|error| McpError::transport(error.to_string()))
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

fn tool_spec_from_rmcp(server_name: &McpServerName, tool: Tool) -> Result<ToolSpec> {
    let approval = tool
        .annotations
        .map(|annotations| {
            ToolApprovalProfile::new(
                annotations.read_only_hint.unwrap_or(false),
                annotations.destructive_hint.unwrap_or(false),
                annotations.idempotent_hint,
                annotations.open_world_hint.unwrap_or(false),
            )
        })
        .unwrap_or_default();

    Ok(ToolSpec {
        name: tool.name.to_string().into(),
        description: tool
            .description
            .map(|value| value.to_string())
            .unwrap_or_default(),
        kind: ToolKind::Function,
        input_schema: Some(Value::Object((*tool.input_schema).clone())),
        freeform_format: None,
        output_mode: ToolOutputMode::ContentParts,
        output_schema: tool
            .output_schema
            .map(|schema| Value::Object((*schema).clone())),
        defer_loading: false,
        origin: ToolOrigin::Mcp {
            server_name: server_name.clone(),
        },
        source: ToolSource::McpTool {
            server_name: server_name.clone(),
        },
        aliases: Vec::new(),
        supports_parallel_tool_calls: false,
        availability: Default::default(),
        approval,
    })
}

fn tool_result_from_rmcp(tool_name: &str, result: rmcp::model::CallToolResult) -> ToolResult {
    ToolResult {
        id: ToolCallId::new(),
        call_id: new_opaque_id().into(),
        tool_name: tool_name.to_string().into(),
        parts: rmcp_content_to_parts(result.content),
        attachments: Vec::new(),
        structured_content: result.structured_content,
        continuation: None,
        metadata: serde_json::to_value(result.meta).ok(),
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

fn mcp_resource_template_from_listing(
    template: rmcp::model::ResourceTemplate,
) -> McpResourceTemplate {
    McpResourceTemplate {
        uri_template: template.uri_template.clone(),
        name: template.name.clone(),
        title: template.title.clone(),
        description: template.description.clone().unwrap_or_default(),
        mime_type: template.mime_type.clone(),
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

#[cfg(test)]
mod tests {
    use super::run_indexed_tasks_ordered;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::time::{Duration, sleep};

    #[tokio::test]
    async fn indexed_runner_preserves_input_order() {
        let tasks = (0usize..6)
            .map(|index| async move {
                let delay = (6 - index) as u64 * 5;
                sleep(Duration::from_millis(delay)).await;
                Ok::<_, ()>((index, format!("item-{index}")))
            })
            .collect::<Vec<_>>();

        let values = run_indexed_tasks_ordered(tasks, 3).await.unwrap();
        assert_eq!(
            values,
            vec![
                "item-0".to_string(),
                "item-1".to_string(),
                "item-2".to_string(),
                "item-3".to_string(),
                "item-4".to_string(),
                "item-5".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn indexed_runner_respects_concurrency_bound() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let tasks = (0usize..12)
            .map(|index| {
                let active = active.clone();
                let peak = peak.clone();
                async move {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    update_peak(&peak, now);
                    sleep(Duration::from_millis(10)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok::<_, ()>((index, index))
                }
            })
            .collect::<Vec<_>>();

        let values = run_indexed_tasks_ordered(tasks, 3).await.unwrap();
        assert_eq!(values, (0usize..12).collect::<Vec<_>>());
        assert!(peak.load(Ordering::SeqCst) <= 3);
    }

    fn update_peak(peak: &AtomicUsize, candidate: usize) {
        let mut current = peak.load(Ordering::SeqCst);
        while candidate > current {
            match peak.compare_exchange(current, candidate, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
    }
}
