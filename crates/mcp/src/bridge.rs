use crate::{ConnectedMcpServer, McpClient, McpResource, McpResourceTemplate, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use tools::{
    DynamicTool, DynamicToolHandler, HOST_FEATURE_HOST_PROCESS_SURFACES, McpToolAdapter,
    Result as ToolsResult, ToolError, ToolExecutionContext,
};
use types::{
    CallId, McpServerName, McpToolBoundary, McpToolBoundaryClass, MessagePart, ToolApprovalProfile,
    ToolAvailability, ToolCallId, ToolContinuation, ToolOrigin, ToolOutputMode, ToolResult,
    ToolSource, ToolSpec,
};

const DEFAULT_MCP_RESOURCE_MAX_CHARS: usize = 32 * 1024;
const MAX_MCP_RESOURCE_MAX_CHARS: usize = 256 * 1024;

#[derive(Clone, Debug, Deserialize)]
struct ListMcpResourcesInput {
    #[serde(default)]
    server_name: Option<McpServerName>,
    #[serde(default)]
    uri_prefix: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct McpResourceRecord {
    server_name: McpServerName,
    uri: String,
    name: String,
    title: Option<String>,
    description: String,
    mime_type: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct ListMcpResourceTemplatesInput {
    #[serde(default)]
    server_name: Option<McpServerName>,
    #[serde(default)]
    uri_template_prefix: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct McpResourceTemplateRecord {
    server_name: McpServerName,
    uri_template: String,
    name: String,
    title: Option<String>,
    description: String,
    mime_type: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct ListMcpResourcesOutput {
    server_name: Option<McpServerName>,
    uri_prefix: Option<String>,
    result_count: usize,
    resources: Vec<McpResourceRecord>,
}

#[derive(Clone, Debug, Serialize)]
struct ListMcpResourceTemplatesOutput {
    server_name: Option<McpServerName>,
    uri_template_prefix: Option<String>,
    result_count: usize,
    resource_templates: Vec<McpResourceTemplateRecord>,
}

#[derive(Clone, Debug, Deserialize)]
struct ReadMcpResourceInput {
    server_name: McpServerName,
    uri: String,
    #[serde(default)]
    start_index: Option<usize>,
    #[serde(default)]
    max_chars: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ReadMcpResourceOutput {
    TextWindow {
        server_name: McpServerName,
        uri: String,
        mime_type: Option<String>,
        document_id: String,
        start_index: usize,
        end_index: usize,
        returned_chars: usize,
        total_chars: usize,
        remaining_chars: usize,
        next_start_index: Option<usize>,
        preview_text: String,
    },
    ContentParts {
        server_name: McpServerName,
        uri: String,
        mime_type: Option<String>,
        part_count: usize,
    },
}

pub async fn catalog_tools_as_registry_entries(
    client: Arc<dyn McpClient>,
) -> Result<Vec<McpToolAdapter>> {
    let catalog = client.catalog().await?;
    let adapters = catalog
        .tools
        .into_iter()
        .map(|spec| {
            let tool_name = spec.name.clone();
            let client = client.clone();
            McpToolAdapter::new(
                spec,
                Arc::new(move |call_id: ToolCallId, arguments: Value| {
                    let client = client.clone();
                    let tool_name = tool_name.clone();
                    Box::pin(async move {
                        client
                            .call_tool(tool_name.as_str(), arguments)
                            .await
                            .map_err(|error| ToolError::invalid_state(error.to_string()))
                            .map(|mut tool_result: ToolResult| {
                                tool_result.id = call_id;
                                tool_result
                            })
                    })
                }),
            )
        })
        .collect();
    Ok(adapters)
}

pub fn catalog_resource_tools_as_registry_entries(
    servers: Vec<ConnectedMcpServer>,
) -> Vec<DynamicTool> {
    // MCP resources stay behind one shared list/read pair so the registry
    // surface remains stable as servers connect, disconnect, or refresh their
    // catalog. The server and resource identity then travel as normal tool
    // arguments and result metadata instead of exploding into per-server tools.
    let has_resource_surfaces = servers.iter().any(|server| {
        !server.catalog.resources.is_empty() || !server.catalog.resource_templates.is_empty()
    });
    if !has_resource_surfaces {
        return Vec::new();
    }

    vec![
        build_list_mcp_resources_tool(servers.clone()),
        build_list_mcp_resource_templates_tool(servers.clone()),
        build_read_mcp_resource_tool(servers),
    ]
}

fn build_list_mcp_resources_tool(servers: Vec<ConnectedMcpServer>) -> DynamicTool {
    let server_boundaries = mcp_server_boundaries(&servers);
    let spec = ToolSpec::function(
        "list_mcp_resources",
        "List MCP resources exposed by connected servers. Supports optional filtering by server name and URI prefix.",
        list_mcp_resources_input_schema(),
        ToolOutputMode::Text,
        ToolOrigin::Mcp {
            server_name: "*".into(),
        },
        ToolSource::McpResource {
            server_name: "*".into(),
        },
    )
    .with_output_schema(list_mcp_resources_output_schema())
    .with_parallel_support(true)
    .with_availability(aggregate_resource_tool_availability(&servers))
    .with_mcp_server_boundaries(server_boundaries)
    .with_approval(ToolApprovalProfile::new(true, false, Some(true), false));
    let handler: DynamicToolHandler = Arc::new(move |call_id, arguments, ctx| {
        let servers = servers.clone();
        Box::pin(async move { execute_list_mcp_resources(call_id, arguments, &servers, &ctx) })
    });
    DynamicTool::from_tool_spec(spec, handler)
}

fn build_read_mcp_resource_tool(servers: Vec<ConnectedMcpServer>) -> DynamicTool {
    let server_boundaries = mcp_server_boundaries(&servers);
    let spec = ToolSpec::function(
        "read_mcp_resource",
        "Read one MCP resource from a connected server. Text-like resources return a paged text window; binary-like resources return content parts.",
        read_mcp_resource_input_schema(),
        ToolOutputMode::ContentParts,
        ToolOrigin::Mcp {
            server_name: "*".into(),
        },
        ToolSource::McpResource {
            server_name: "*".into(),
        },
    )
    .with_output_schema(read_mcp_resource_output_schema())
    .with_parallel_support(true)
    .with_availability(aggregate_resource_tool_availability(&servers))
    .with_mcp_server_boundaries(server_boundaries)
    .with_approval(ToolApprovalProfile::new(true, false, Some(true), true).with_network(true));
    let handler: DynamicToolHandler = Arc::new(move |call_id, arguments, ctx| {
        let servers = servers.clone();
        Box::pin(async move { execute_read_mcp_resource(call_id, arguments, &servers, &ctx).await })
    });
    DynamicTool::from_tool_spec(spec, handler)
}

fn build_list_mcp_resource_templates_tool(servers: Vec<ConnectedMcpServer>) -> DynamicTool {
    let server_boundaries = mcp_server_boundaries(&servers);
    let spec = ToolSpec::function(
        "list_mcp_resource_templates",
        "List MCP resource templates exposed by connected servers. Supports optional filtering by server name and URI template prefix.",
        list_mcp_resource_templates_input_schema(),
        ToolOutputMode::Text,
        ToolOrigin::Mcp {
            server_name: "*".into(),
        },
        ToolSource::McpResource {
            server_name: "*".into(),
        },
    )
    .with_output_schema(list_mcp_resource_templates_output_schema())
    .with_parallel_support(true)
    .with_availability(aggregate_resource_tool_availability(&servers))
    .with_mcp_server_boundaries(server_boundaries)
    .with_approval(ToolApprovalProfile::new(true, false, Some(true), false));
    let handler: DynamicToolHandler = Arc::new(move |call_id, arguments, ctx| {
        let servers = servers.clone();
        Box::pin(
            async move { execute_list_mcp_resource_templates(call_id, arguments, &servers, &ctx) },
        )
    });
    DynamicTool::from_tool_spec(spec, handler)
}

fn boundary_requires_host_process(boundary: &McpToolBoundary) -> bool {
    matches!(boundary.boundary_class, McpToolBoundaryClass::LocalProcess)
}

fn server_requires_host_process(server: &ConnectedMcpServer) -> bool {
    boundary_requires_host_process(&server.boundary)
}

fn server_is_visible(ctx: &ToolExecutionContext, server: &ConnectedMcpServer) -> bool {
    ctx.is_mcp_server_allowed(&server.server_name)
        && (!server_requires_host_process(server)
            || ctx
                .model_visibility
                .has_feature(HOST_FEATURE_HOST_PROCESS_SURFACES))
}

fn aggregate_resource_tool_availability(servers: &[ConnectedMcpServer]) -> ToolAvailability {
    if !servers.is_empty() && servers.iter().all(server_requires_host_process) {
        return ToolAvailability {
            feature_flags: vec![HOST_FEATURE_HOST_PROCESS_SURFACES.to_string()],
            ..ToolAvailability::default()
        };
    }

    ToolAvailability::default()
}

fn execute_list_mcp_resources(
    call_id: ToolCallId,
    arguments: Value,
    servers: &[ConnectedMcpServer],
    ctx: &ToolExecutionContext,
) -> ToolsResult<ToolResult> {
    let external_call_id = CallId::from(&call_id);
    let input: ListMcpResourcesInput = serde_json::from_value(arguments)?;
    if let Some(server_name) = input.server_name.as_ref() {
        ctx.assert_mcp_server_allowed(server_name)?;
    }
    let resources = servers
        .iter()
        .filter(|server| server_is_visible(ctx, server))
        .filter(|server| {
            input
                .server_name
                .as_ref()
                .is_none_or(|server_name| server.server_name == *server_name)
        })
        .flat_map(|server| {
            server.catalog.resources.iter().filter_map(|resource| {
                if let Some(prefix) = input.uri_prefix.as_deref()
                    && !resource.uri.starts_with(prefix)
                {
                    return None;
                }
                Some(McpResourceRecord {
                    server_name: server.server_name.clone(),
                    uri: resource.uri.clone(),
                    name: resource.name.clone(),
                    title: resource.title.clone(),
                    description: resource.description.clone(),
                    mime_type: resource.mime_type.clone(),
                })
            })
        })
        .collect::<Vec<_>>();
    let text = if resources.is_empty() {
        "No MCP resources matched the current filters.".to_string()
    } else {
        resources
            .iter()
            .map(format_resource_record)
            .collect::<Vec<_>>()
            .join("\n")
    };
    let structured_output = ListMcpResourcesOutput {
        server_name: input.server_name.clone(),
        uri_prefix: input.uri_prefix.clone(),
        result_count: resources.len(),
        resources: resources.clone(),
    };

    Ok(ToolResult {
        id: call_id,
        call_id: external_call_id,
        tool_name: "list_mcp_resources".into(),
        parts: vec![MessagePart::text(text)],
        attachments: Vec::new(),
        structured_content: Some(
            serde_json::to_value(structured_output).expect("list_mcp_resources structured output"),
        ),
        continuation: None,
        metadata: Some(json!({
            "server_name": input.server_name,
            "uri_prefix": input.uri_prefix,
            "result_count": resources.len(),
        })),
        is_error: false,
    })
}

fn mcp_server_boundaries(
    servers: &[ConnectedMcpServer],
) -> BTreeMap<McpServerName, McpToolBoundary> {
    servers
        .iter()
        .map(|server| (server.server_name.clone(), server.boundary.clone()))
        .collect()
}

fn execute_list_mcp_resource_templates(
    call_id: ToolCallId,
    arguments: Value,
    servers: &[ConnectedMcpServer],
    ctx: &ToolExecutionContext,
) -> ToolsResult<ToolResult> {
    let external_call_id = CallId::from(&call_id);
    let input: ListMcpResourceTemplatesInput = serde_json::from_value(arguments)?;
    if let Some(server_name) = input.server_name.as_ref() {
        ctx.assert_mcp_server_allowed(server_name)?;
    }
    let resource_templates = servers
        .iter()
        .filter(|server| server_is_visible(ctx, server))
        .filter(|server| {
            input
                .server_name
                .as_ref()
                .is_none_or(|server_name| server.server_name == *server_name)
        })
        .flat_map(|server| {
            server
                .catalog
                .resource_templates
                .iter()
                .filter_map(|template| {
                    if let Some(prefix) = input.uri_template_prefix.as_deref()
                        && !template.uri_template.starts_with(prefix)
                    {
                        return None;
                    }
                    Some(McpResourceTemplateRecord::from_template(
                        server.server_name.clone(),
                        template,
                    ))
                })
        })
        .collect::<Vec<_>>();
    let text = if resource_templates.is_empty() {
        "No MCP resource templates matched the current filters.".to_string()
    } else {
        resource_templates
            .iter()
            .map(format_resource_template_record)
            .collect::<Vec<_>>()
            .join("\n")
    };
    let structured_output = ListMcpResourceTemplatesOutput {
        server_name: input.server_name.clone(),
        uri_template_prefix: input.uri_template_prefix.clone(),
        result_count: resource_templates.len(),
        resource_templates: resource_templates.clone(),
    };

    Ok(ToolResult {
        id: call_id,
        call_id: external_call_id,
        tool_name: "list_mcp_resource_templates".into(),
        parts: vec![MessagePart::text(text)],
        attachments: Vec::new(),
        structured_content: Some(
            serde_json::to_value(structured_output)
                .expect("list_mcp_resource_templates structured output"),
        ),
        continuation: None,
        metadata: Some(json!({
            "server_name": input.server_name,
            "uri_template_prefix": input.uri_template_prefix,
            "result_count": resource_templates.len(),
        })),
        is_error: false,
    })
}

async fn execute_read_mcp_resource(
    call_id: ToolCallId,
    arguments: Value,
    servers: &[ConnectedMcpServer],
    ctx: &ToolExecutionContext,
) -> ToolsResult<ToolResult> {
    let external_call_id = CallId::from(&call_id);
    let input: ReadMcpResourceInput = serde_json::from_value(arguments)?;
    ctx.assert_mcp_server_allowed(&input.server_name)?;
    let server = servers
        .iter()
        .find(|server| server.server_name == input.server_name)
        .ok_or_else(|| ToolError::invalid(format!("unknown MCP server `{}`", input.server_name)))?;
    if !server_is_visible(ctx, server) {
        return Err(ToolError::invalid_state(format!(
            "MCP server `{}` is unavailable while host process surfaces are disabled",
            input.server_name
        )));
    }
    let resource = server
        .client
        .read_resource(&input.uri)
        .await
        .map_err(|error| ToolError::invalid_state(error.to_string()))?;

    if let Some(text) = extract_text_resource(&resource) {
        // Text-like resources use the same document-window continuation shape as
        // paged web fetches so follow-up reads can continue from a stable cursor
        // without depending on the original MCP part layout.
        let max_chars = input
            .max_chars
            .unwrap_or(DEFAULT_MCP_RESOURCE_MAX_CHARS)
            .clamp(1, MAX_MCP_RESOURCE_MAX_CHARS);
        let total_chars = text.chars().count();
        let start_index = input.start_index.unwrap_or(0).min(total_chars);
        let tail = text.chars().skip(start_index).collect::<String>();
        let (preview, truncated) = truncate_text(&tail, max_chars);
        let returned_chars = preview.chars().count();
        let end_index = start_index + returned_chars;
        let remaining_chars = total_chars.saturating_sub(end_index);
        let next_start_index = truncated.then_some(end_index);
        let document_id = format!("mcp_resource:{}:{}", server.server_name, resource.uri);
        let structured_output = ReadMcpResourceOutput::TextWindow {
            server_name: server.server_name.clone(),
            uri: resource.uri.clone(),
            mime_type: resource.mime_type.clone(),
            document_id: document_id.clone(),
            start_index,
            end_index,
            returned_chars,
            total_chars,
            remaining_chars,
            next_start_index,
            preview_text: preview.clone(),
        };
        let mut sections = vec![
            format!("server> {}", server.server_name),
            format!("uri> {}", resource.uri),
            format!(
                "mime_type> {}",
                resource.mime_type.as_deref().unwrap_or("unknown")
            ),
            format!("start_index> {start_index}"),
            format!("end_index> {end_index}"),
            format!("total_chars> {total_chars}"),
            String::new(),
            preview,
        ];
        if let Some(next_start_index) = next_start_index {
            sections.push(format!(
                "\n[truncated to {max_chars} characters; continue with start_index={next_start_index}]"
            ));
        }

        return Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "read_mcp_resource".into(),
            parts: vec![MessagePart::text(sections.join("\n"))],
            attachments: Vec::new(),
            structured_content: Some(
                serde_json::to_value(structured_output)
                    .expect("read_mcp_resource text structured output"),
            ),
            continuation: Some(ToolContinuation::DocumentWindow {
                document_id,
                next_start_index,
            }),
            metadata: Some(json!({
                "server_name": server.server_name,
                "uri": resource.uri,
                "mime_type": resource.mime_type,
                "start_index": start_index,
                "end_index": end_index,
                "total_chars": total_chars,
                "remaining_chars": remaining_chars,
                "next_start_index": next_start_index,
            })),
            is_error: false,
        });
    }

    let summary = format!(
        "server> {}\nuri> {}\nmime_type> {}\nparts> {}\n\n[non-text MCP resource returned as content parts]",
        server.server_name,
        resource.uri,
        resource.mime_type.as_deref().unwrap_or("unknown"),
        resource.parts.len()
    );
    Ok(ToolResult {
        id: call_id,
        call_id: external_call_id,
        tool_name: "read_mcp_resource".into(),
        parts: std::iter::once(MessagePart::text(summary))
            .chain(resource.parts.clone())
            .collect(),
        attachments: Vec::new(),
        structured_content: Some(
            serde_json::to_value(ReadMcpResourceOutput::ContentParts {
                server_name: server.server_name.clone(),
                uri: resource.uri.clone(),
                mime_type: resource.mime_type.clone(),
                part_count: resource.parts.len(),
            })
            .expect("read_mcp_resource content structured output"),
        ),
        continuation: None,
        metadata: Some(json!({
            "server_name": server.server_name,
            "uri": resource.uri,
            "mime_type": resource.mime_type,
            "part_count": resource.parts.len(),
        })),
        is_error: false,
    })
}

fn list_mcp_resources_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "server_name": { "type": "string" },
            "uri_prefix": { "type": "string" }
        }
    })
}

fn list_mcp_resources_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "server_name": { "type": "string" },
            "uri_prefix": { "type": "string" },
            "result_count": { "type": "integer" },
            "resources": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "server_name": { "type": "string" },
                        "uri": { "type": "string" },
                        "name": { "type": "string" },
                        "title": { "type": "string" },
                        "description": { "type": "string" },
                        "mime_type": { "type": "string" }
                    },
                    "required": ["server_name", "uri", "name", "description"]
                }
            }
        },
        "required": ["result_count", "resources"]
    })
}

fn list_mcp_resource_templates_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "server_name": { "type": "string" },
            "uri_template_prefix": { "type": "string" }
        }
    })
}

fn list_mcp_resource_templates_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "server_name": { "type": "string" },
            "uri_template_prefix": { "type": "string" },
            "result_count": { "type": "integer" },
            "resource_templates": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "server_name": { "type": "string" },
                        "uri_template": { "type": "string" },
                        "name": { "type": "string" },
                        "title": { "type": "string" },
                        "description": { "type": "string" },
                        "mime_type": { "type": "string" }
                    },
                    "required": [
                        "server_name",
                        "uri_template",
                        "name",
                        "description"
                    ]
                }
            }
        },
        "required": ["result_count", "resource_templates"]
    })
}

fn read_mcp_resource_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "server_name": { "type": "string" },
            "uri": { "type": "string" },
            "start_index": { "type": "integer", "minimum": 0 },
            "max_chars": { "type": "integer", "minimum": 1 }
        },
        "required": ["server_name", "uri"]
    })
}

fn read_mcp_resource_output_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "kind": { "const": "text_window" },
                    "server_name": { "type": "string" },
                    "uri": { "type": "string" },
                    "mime_type": { "type": "string" },
                    "document_id": { "type": "string" },
                    "start_index": { "type": "integer" },
                    "end_index": { "type": "integer" },
                    "returned_chars": { "type": "integer" },
                    "total_chars": { "type": "integer" },
                    "remaining_chars": { "type": "integer" },
                    "next_start_index": { "type": "integer" },
                    "preview_text": { "type": "string" }
                },
                "required": [
                    "kind",
                    "server_name",
                    "uri",
                    "document_id",
                    "start_index",
                    "end_index",
                    "returned_chars",
                    "total_chars",
                    "remaining_chars",
                    "preview_text"
                ]
            },
            {
                "type": "object",
                "properties": {
                    "kind": { "const": "content_parts" },
                    "server_name": { "type": "string" },
                    "uri": { "type": "string" },
                    "mime_type": { "type": "string" },
                    "part_count": { "type": "integer" }
                },
                "required": ["kind", "server_name", "uri", "part_count"]
            }
        ]
    })
}

fn format_resource_record(record: &McpResourceRecord) -> String {
    format!(
        "{} {} {}{}",
        record.server_name,
        record.uri,
        record.mime_type.as_deref().unwrap_or("unknown"),
        record
            .title
            .as_deref()
            .map(|title| format!(" {title}"))
            .unwrap_or_default(),
    )
}

impl McpResourceTemplateRecord {
    fn from_template(server_name: McpServerName, template: &McpResourceTemplate) -> Self {
        Self {
            server_name,
            uri_template: template.uri_template.clone(),
            name: template.name.clone(),
            title: template.title.clone(),
            description: template.description.clone(),
            mime_type: template.mime_type.clone(),
        }
    }
}

fn format_resource_template_record(record: &McpResourceTemplateRecord) -> String {
    format!(
        "{} {} {}{}",
        record.server_name,
        record.uri_template,
        record.mime_type.as_deref().unwrap_or("unknown"),
        record
            .title
            .as_deref()
            .map(|title| format!(" {title}"))
            .unwrap_or_default(),
    )
}

fn extract_text_resource(resource: &McpResource) -> Option<String> {
    let parts = resource
        .parts
        .iter()
        .map(textual_message_part)
        .collect::<Option<Vec<_>>>()?;
    let text = parts.join("\n\n").trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn textual_message_part(part: &MessagePart) -> Option<String> {
    match part {
        MessagePart::Text { text } => Some(text.clone()),
        MessagePart::Resource {
            text: Some(text), ..
        } => Some(text.clone()),
        MessagePart::Json { value } => Some(value.to_string()),
        _ => None,
    }
}

fn truncate_text(value: &str, limit: usize) -> (String, bool) {
    let char_count = value.chars().count();
    if char_count <= limit {
        return (value.to_string(), false);
    }
    (value.chars().take(limit).collect::<String>(), true)
}

#[cfg(test)]
mod tests {
    use super::catalog_resource_tools_as_registry_entries;
    use crate::{ConnectedMcpServer, McpCatalog, McpResource, McpResourceTemplate, MockMcpClient};
    use serde_json::json;
    use std::sync::Arc;
    use tools::{HOST_FEATURE_HOST_PROCESS_SURFACES, Tool, ToolExecutionContext};
    use types::{
        McpToolBoundary, McpTransportKind, MessagePart, ToolCallId, ToolContinuation,
        ToolVisibilityContext,
    };

    fn tool_context_with_host_process() -> ToolExecutionContext {
        ToolExecutionContext {
            model_visibility: ToolVisibilityContext::default()
                .with_feature(HOST_FEATURE_HOST_PROCESS_SURFACES),
            ..Default::default()
        }
    }

    fn fixture_markdown_server(
        server_name: &str,
        boundary: McpToolBoundary,
        uri: &str,
        description: &str,
    ) -> ConnectedMcpServer {
        let catalog = McpCatalog {
            server_name: server_name.into(),
            tools: Vec::new(),
            prompts: Vec::new(),
            resources: vec![McpResource {
                uri: uri.to_string(),
                name: "guide".to_string(),
                title: Some("Guide".to_string()),
                description: description.to_string(),
                mime_type: Some("text/markdown".to_string()),
                parts: vec![MessagePart::Resource {
                    uri: uri.to_string(),
                    mime_type: Some("text/markdown".to_string()),
                    text: Some("# Guide\n\nUseful context.".to_string()),
                    metadata: None,
                }],
            }],
            resource_templates: vec![McpResourceTemplate {
                uri_template: format!("{uri}/{{section}}"),
                name: "guide-template".to_string(),
                title: Some("Guide Template".to_string()),
                description: format!("templated {description}"),
                mime_type: Some("text/markdown".to_string()),
            }],
        };
        ConnectedMcpServer {
            server_name: server_name.into(),
            boundary,
            client: Arc::new(MockMcpClient::new(
                catalog.clone(),
                Arc::new(|_, _| {
                    unreachable!("tool handler should not run in resource bridge test")
                }),
            )),
            catalog,
        }
    }

    #[tokio::test]
    async fn resource_bridge_exposes_list_and_read_tools() {
        let server = fixture_markdown_server(
            "fixture",
            McpToolBoundary::local_process(McpTransportKind::Stdio),
            "fixture://guide",
            "fixture guide",
        );

        let tools = catalog_resource_tools_as_registry_entries(vec![server]);
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].spec().name.as_str(), "list_mcp_resources");
        assert_eq!(tools[1].spec().name.as_str(), "list_mcp_resource_templates");
        assert_eq!(tools[2].spec().name.as_str(), "read_mcp_resource");
        assert_eq!(
            tools
                .iter()
                .map(|tool| serde_json::to_value(tool.spec()).unwrap())
                .collect::<Vec<_>>(),
            vec![
                json!({
                    "name": "list_mcp_resources",
                    "description": "List MCP resources exposed by connected servers. Supports optional filtering by server name and URI prefix.",
                    "kind": "function",
                    "input_schema": {
                        "properties": {
                            "server_name": {"type": "string"},
                            "uri_prefix": {"type": "string"}
                        },
                        "type": "object"
                    },
                    "output_mode": "text",
                    "output_schema": {
                        "properties": {
                            "server_name": {"type": "string"},
                            "uri_prefix": {"type": "string"},
                            "result_count": {"type": "integer"},
                            "resources": {
                                "type": "array",
                                "items": {
                                    "properties": {
                                        "server_name": {"type": "string"},
                                        "uri": {"type": "string"},
                                        "name": {"type": "string"},
                                        "title": {"type": "string"},
                                        "description": {"type": "string"},
                                        "mime_type": {"type": "string"}
                                    },
                                    "required": ["server_name", "uri", "name", "description"],
                                    "type": "object"
                                }
                            }
                        },
                        "required": ["result_count", "resources"],
                        "type": "object"
                    },
                    "defer_loading": false,
                    "origin": {"kind": "mcp", "server_name": "*"},
                    "source": {"kind": "mcp_resource", "server_name": "*"},
                    "aliases": [],
                    "supports_parallel_tool_calls": true,
                    "availability": {
                        "feature_flags": ["host-process-surfaces"],
                        "hidden_from_model": false
                    },
                    "approval": {
                        "read_only": true,
                        "mutates_state": false,
                        "idempotent": true,
                        "open_world": false,
                        "needs_network": false,
                        "needs_host_escape": false
                    },
                    "mcp_server_boundaries": {
                        "fixture": {
                            "transport": "stdio",
                            "boundary_class": "local_process"
                        }
                    }
                }),
                json!({
                    "name": "list_mcp_resource_templates",
                    "description": "List MCP resource templates exposed by connected servers. Supports optional filtering by server name and URI template prefix.",
                    "kind": "function",
                    "input_schema": {
                        "properties": {
                            "server_name": {"type": "string"},
                            "uri_template_prefix": {"type": "string"}
                        },
                        "type": "object"
                    },
                    "output_mode": "text",
                    "output_schema": {
                        "properties": {
                            "server_name": {"type": "string"},
                            "uri_template_prefix": {"type": "string"},
                            "result_count": {"type": "integer"},
                            "resource_templates": {
                                "type": "array",
                                "items": {
                                    "properties": {
                                        "server_name": {"type": "string"},
                                        "uri_template": {"type": "string"},
                                        "name": {"type": "string"},
                                        "title": {"type": "string"},
                                        "description": {"type": "string"},
                                        "mime_type": {"type": "string"}
                                    },
                                    "required": ["server_name", "uri_template", "name", "description"],
                                    "type": "object"
                                }
                            }
                        },
                        "required": ["result_count", "resource_templates"],
                        "type": "object"
                    },
                    "defer_loading": false,
                    "origin": {"kind": "mcp", "server_name": "*"},
                    "source": {"kind": "mcp_resource", "server_name": "*"},
                    "aliases": [],
                    "supports_parallel_tool_calls": true,
                    "availability": {
                        "feature_flags": ["host-process-surfaces"],
                        "hidden_from_model": false
                    },
                    "approval": {
                        "read_only": true,
                        "mutates_state": false,
                        "idempotent": true,
                        "open_world": false,
                        "needs_network": false,
                        "needs_host_escape": false
                    },
                    "mcp_server_boundaries": {
                        "fixture": {
                            "transport": "stdio",
                            "boundary_class": "local_process"
                        }
                    }
                }),
                json!({
                    "name": "read_mcp_resource",
                    "description": "Read one MCP resource from a connected server. Text-like resources return a paged text window; binary-like resources return content parts.",
                    "kind": "function",
                    "input_schema": {
                        "properties": {
                            "max_chars": {"minimum": 1, "type": "integer"},
                            "server_name": {"type": "string"},
                            "start_index": {"minimum": 0, "type": "integer"},
                            "uri": {"type": "string"}
                        },
                        "required": ["server_name", "uri"],
                        "type": "object"
                    },
                    "output_mode": "content_parts",
                    "output_schema": {
                        "oneOf": [
                            {
                                "properties": {
                                    "kind": {"const": "text_window"},
                                    "server_name": {"type": "string"},
                                    "uri": {"type": "string"},
                                    "mime_type": {"type": "string"},
                                    "document_id": {"type": "string"},
                                    "start_index": {"type": "integer"},
                                    "end_index": {"type": "integer"},
                                    "returned_chars": {"type": "integer"},
                                    "total_chars": {"type": "integer"},
                                    "remaining_chars": {"type": "integer"},
                                    "next_start_index": {"type": "integer"},
                                    "preview_text": {"type": "string"}
                                },
                                "required": ["kind", "server_name", "uri", "document_id", "start_index", "end_index", "returned_chars", "total_chars", "remaining_chars", "preview_text"],
                                "type": "object"
                            },
                            {
                                "properties": {
                                    "kind": {"const": "content_parts"},
                                    "uri": {"type": "string"},
                                    "mime_type": {"type": "string"},
                                    "part_count": {"type": "integer"},
                                    "server_name": {"type": "string"}
                                },
                                "required": ["kind", "server_name", "uri", "part_count"],
                                "type": "object"
                            }
                        ]
                    },
                    "defer_loading": false,
                    "origin": {"kind": "mcp", "server_name": "*"},
                    "source": {"kind": "mcp_resource", "server_name": "*"},
                    "aliases": [],
                    "supports_parallel_tool_calls": true,
                    "availability": {
                        "feature_flags": ["host-process-surfaces"],
                        "hidden_from_model": false
                    },
                    "approval": {
                        "read_only": true,
                        "mutates_state": false,
                        "idempotent": true,
                        "open_world": true,
                        "needs_network": true,
                        "needs_host_escape": false
                    },
                    "mcp_server_boundaries": {
                        "fixture": {
                            "transport": "stdio",
                            "boundary_class": "local_process"
                        }
                    }
                })
            ]
        );
        assert_eq!(
            tools[2].spec().effective_mcp_boundary(&types::ToolCall {
                id: ToolCallId::new(),
                call_id: "approval-check".into(),
                tool_name: "read_mcp_resource".into(),
                arguments: json!({"server_name": "fixture", "uri": "fixture://guide"}),
                origin: types::ToolOrigin::Mcp {
                    server_name: "*".into(),
                },
            }),
            Some(&McpToolBoundary::local_process(McpTransportKind::Stdio))
        );

        let list = tools[0]
            .execute(
                ToolCallId::from("list-call"),
                json!({}),
                &tool_context_with_host_process(),
            )
            .await
            .unwrap();
        assert_eq!(list.structured_content.unwrap()["result_count"], json!(1));

        let templates = tools[1]
            .execute(
                ToolCallId::from("template-call"),
                json!({}),
                &tool_context_with_host_process(),
            )
            .await
            .unwrap();
        assert_eq!(
            templates.structured_content.unwrap()["result_count"],
            json!(1)
        );

        let read = tools[2]
            .execute(
                ToolCallId::from("read-call"),
                json!({
                    "server_name": "fixture",
                    "uri": "fixture://guide",
                    "max_chars": 8
                }),
                &tool_context_with_host_process(),
            )
            .await
            .unwrap();
        assert_eq!(
            read.continuation,
            Some(ToolContinuation::DocumentWindow {
                document_id: "mcp_resource:fixture:fixture://guide".to_string(),
                next_start_index: Some(8),
            })
        );
        assert_eq!(
            read.structured_content.unwrap()["kind"],
            json!("text_window")
        );
    }

    #[tokio::test]
    async fn resource_bridge_hides_local_process_servers_without_host_process_feature() {
        let local_server = fixture_markdown_server(
            "local",
            McpToolBoundary::local_process(McpTransportKind::Stdio),
            "local://guide",
            "local fixture guide",
        );
        let remote_server = fixture_markdown_server(
            "remote",
            McpToolBoundary::remote_service(McpTransportKind::StreamableHttp),
            "remote://guide",
            "remote fixture guide",
        );

        let tools = catalog_resource_tools_as_registry_entries(vec![local_server, remote_server]);
        assert!(tools[0].spec().availability.feature_flags.is_empty());

        let list_without_feature = tools[0]
            .execute(
                ToolCallId::from("list-without-feature"),
                json!({}),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            list_without_feature.structured_content.as_ref().unwrap()["result_count"],
            json!(1)
        );
        assert_eq!(
            list_without_feature.structured_content.as_ref().unwrap()["resources"][0]["server_name"],
            json!("remote")
        );

        let read_local_without_feature = tools[2]
            .execute(
                ToolCallId::from("read-local-without-feature"),
                json!({
                    "server_name": "local",
                    "uri": "local://guide"
                }),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            read_local_without_feature.to_string(),
            "tool state error: MCP server `local` is unavailable while host process surfaces are disabled"
        );

        let list_with_feature = tools[0]
            .execute(
                ToolCallId::from("list-with-feature"),
                json!({}),
                &tool_context_with_host_process(),
            )
            .await
            .unwrap();
        assert_eq!(
            list_with_feature.structured_content.as_ref().unwrap()["result_count"],
            json!(2)
        );

        let read_local_with_feature = tools[2]
            .execute(
                ToolCallId::from("read-local-with-feature"),
                json!({
                    "server_name": "local",
                    "uri": "local://guide"
                }),
                &tool_context_with_host_process(),
            )
            .await
            .unwrap();
        assert_eq!(
            read_local_with_feature.structured_content.unwrap()["server_name"],
            json!("local")
        );
    }

    #[tokio::test]
    async fn resource_bridge_reads_non_text_resources_as_content_parts() {
        let server = ConnectedMcpServer {
            server_name: "fixture".into(),
            boundary: McpToolBoundary::remote_service(McpTransportKind::StreamableHttp),
            client: Arc::new(MockMcpClient::new(
                McpCatalog {
                    server_name: "fixture".into(),
                    tools: Vec::new(),
                    prompts: Vec::new(),
                    resources: vec![McpResource {
                        uri: "fixture://binary".to_string(),
                        name: "binary".to_string(),
                        title: None,
                        description: "fixture binary".to_string(),
                        mime_type: Some("application/octet-stream".to_string()),
                        parts: vec![MessagePart::File {
                            file_name: Some("binary.bin".to_string()),
                            mime_type: Some("application/octet-stream".to_string()),
                            data_base64: None,
                            uri: Some("fixture://binary".to_string()),
                        }],
                    }],
                    resource_templates: Vec::new(),
                },
                Arc::new(|_, _| {
                    unreachable!("tool handler should not run in resource bridge test")
                }),
            )),
            catalog: McpCatalog {
                server_name: "fixture".into(),
                tools: Vec::new(),
                prompts: Vec::new(),
                resources: vec![McpResource {
                    uri: "fixture://binary".to_string(),
                    name: "binary".to_string(),
                    title: None,
                    description: "fixture binary".to_string(),
                    mime_type: Some("application/octet-stream".to_string()),
                    parts: vec![MessagePart::File {
                        file_name: Some("binary.bin".to_string()),
                        mime_type: Some("application/octet-stream".to_string()),
                        data_base64: None,
                        uri: Some("fixture://binary".to_string()),
                    }],
                }],
                resource_templates: Vec::new(),
            },
        };

        let tools = catalog_resource_tools_as_registry_entries(vec![server]);
        let read = tools[2]
            .execute(
                ToolCallId::from("read-call"),
                json!({
                    "server_name": "fixture",
                    "uri": "fixture://binary"
                }),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        assert!(read.continuation.is_none());
        assert_eq!(
            read.structured_content.unwrap()["kind"],
            json!("content_parts")
        );
        assert!(matches!(read.parts.first(), Some(MessagePart::Text { .. })));
        assert!(matches!(read.parts.get(1), Some(MessagePart::File { .. })));
    }
}
