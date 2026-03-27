use anyhow::{Result, anyhow};
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    model::{
        AnnotateAble, CallToolRequestParams, CallToolResult, Content, GetPromptRequestParams,
        GetPromptResult, ListPromptsResult, ListResourcesResult, ListToolsResult, Meta,
        PaginatedRequestParams, Prompt, PromptArgument, PromptMessage, PromptMessageRole,
        RawResource, ReadResourceRequestParams, ReadResourceResult, ResourceContents,
        ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
    },
    service::RequestContext,
    transport::stdio,
};
use serde_json::{Map, Value, json};

#[derive(Default)]
struct FixtureServer;

impl ServerHandler for FixtureServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .enable_resources()
                .build(),
        )
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = std::result::Result<ListToolsResult, McpError>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(vec![fixture_tool()])))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = std::result::Result<CallToolResult, McpError>> + Send + '_ {
        async move {
            match request.name.as_ref() {
                "inspect_context" => {
                    let subject = string_argument(request.arguments.as_ref(), "subject")
                        .unwrap_or_else(|| "default".to_string());
                    let cwd = std::env::current_dir()
                        .map_err(|error| McpError::internal_error(error.to_string(), None))?
                        .display()
                        .to_string();
                    let fixture_env = std::env::var("FIXTURE_ENV").unwrap_or_default();
                    let structured = json!({
                        "subject": subject,
                        "cwd": cwd,
                        "fixture_env": fixture_env,
                    });
                    let resource = RawResource::new("fixture://tool-link", "tool-link")
                        .with_description("linked from tool result");
                    Ok(CallToolResult::success(vec![
                        Content::text(structured.to_string()),
                        Content::resource_link(resource),
                    ])
                    .with_meta(Some(Meta(
                        structured.as_object().cloned().ok_or_else(|| {
                            McpError::internal_error(
                                "fixture structured payload must be an object".to_string(),
                                None,
                            )
                        })?,
                    ))))
                }
                other => Err(McpError::invalid_params(
                    format!("unknown fixture tool: {other}"),
                    None,
                )),
            }
        }
    }

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = std::result::Result<ListPromptsResult, McpError>> + Send + '_ {
        std::future::ready(Ok(ListPromptsResult::with_all_items(
            vec![fixture_prompt()],
        )))
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = std::result::Result<GetPromptResult, McpError>> + Send + '_ {
        async move {
            if request.name != "draft_brief" {
                return Err(McpError::invalid_params(
                    format!("unknown fixture prompt: {}", request.name),
                    None,
                ));
            }

            let subject = string_argument(request.arguments.as_ref(), "subject")
                .unwrap_or_else(|| "default".to_string());
            Ok(GetPromptResult::new(vec![
                PromptMessage::new_text(
                    PromptMessageRole::User,
                    format!("Draft a brief for {subject}."),
                ),
                PromptMessage::new_resource(
                    PromptMessageRole::Assistant,
                    "fixture://prompt-resource".to_string(),
                    Some("text/plain".to_string()),
                    Some(format!("context for {subject}")),
                    None,
                    None,
                    None,
                ),
            ])
            .with_description("Fixture drafting prompt"))
        }
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = std::result::Result<ListResourcesResult, McpError>> + Send + '_ {
        std::future::ready(Ok(ListResourcesResult::with_all_items(vec![
            RawResource::new("fixture://guide", "guide")
                .with_title("Fixture Guide")
                .with_description("fixture markdown resource")
                .with_mime_type("text/markdown")
                .no_annotation(),
        ])))
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = std::result::Result<ReadResourceResult, McpError>> + Send + '_ {
        async move {
            if request.uri != "fixture://guide" {
                return Err(McpError::invalid_params(
                    format!("unknown fixture resource: {}", request.uri),
                    None,
                ));
            }

            Ok(ReadResourceResult::new(vec![
                ResourceContents::text("# Fixture Guide\n\nThis is fixture content.", request.uri)
                    .with_mime_type("text/markdown"),
            ]))
        }
    }
}

fn fixture_tool() -> Tool {
    Tool::new(
        "inspect_context",
        "Return the current cwd, configured fixture env var, and request subject.",
        json_object(json!({
            "type": "object",
            "properties": {
                "subject": { "type": "string" }
            },
            "required": ["subject"]
        })),
    )
    .with_title("Inspect Context")
    .with_annotations(
        ToolAnnotations::with_title("Inspect Context")
            .read_only(true)
            .destructive(false)
            .idempotent(true)
            .open_world(false),
    )
    .with_raw_output_schema(json_object(json!({
        "type": "object",
        "properties": {
            "subject": { "type": "string" },
            "cwd": { "type": "string" },
            "fixture_env": { "type": "string" }
        },
        "required": ["subject", "cwd", "fixture_env"]
    })))
}

fn fixture_prompt() -> Prompt {
    Prompt::new(
        "draft_brief",
        Some("Draft a short brief with extra fixture context."),
        Some(vec![
            PromptArgument::new("subject")
                .with_title("Subject")
                .with_description("The subject to draft about.")
                .with_required(true),
        ]),
    )
    .with_title("Draft Brief")
}

fn json_object(value: Value) -> std::sync::Arc<Map<String, Value>> {
    std::sync::Arc::new(
        value
            .as_object()
            .cloned()
            .ok_or_else(|| anyhow!("expected JSON object"))
            .unwrap(),
    )
}

fn string_argument(arguments: Option<&Map<String, Value>>, key: &str) -> Option<String> {
    arguments
        .and_then(|map| map.get(key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

pub async fn run_stdio_fixture_server() -> Result<()> {
    FixtureServer.serve(stdio()).await?.waiting().await?;
    Ok(())
}
