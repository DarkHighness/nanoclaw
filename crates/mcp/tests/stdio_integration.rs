use mcp::{
    McpConnectOptions, McpServerConfig, McpTransportConfig,
    connect_and_catalog_mcp_servers_with_options,
};
use sandbox::Result as SandboxResult;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;
use tokio::process::Command;
use tokio::time::{Duration, timeout};
use tools::{ExecRequest, HostProcessExecutor, ProcessExecutor};
use types::{McpToolBoundary, McpTransportKind, MessagePart, ToolOrigin};

#[derive(Clone)]
struct RecordingExecutor {
    inner: Arc<dyn ProcessExecutor>,
    requests: Arc<Mutex<Vec<ExecRequest>>>,
}

impl ProcessExecutor for RecordingExecutor {
    // Preserve the real process preparation path and only layer request
    // recording on top so the integration test still exercises host startup.
    fn prepare(&self, request: ExecRequest) -> SandboxResult<Command> {
        self.requests.lock().unwrap().push(request.clone());
        self.inner.prepare(request)
    }
}

#[tokio::test]
async fn stdio_server_supports_catalog_tool_prompt_and_resource_round_trips() {
    let fixture_cwd = tempdir().unwrap();
    let expected_cwd = fixture_cwd.path().canonicalize().unwrap();
    let config = McpServerConfig {
        name: "fixture".into(),
        enabled: true,
        bootstrap_network: None,
        runtime_network: None,
        transport: McpTransportConfig::Stdio {
            command: env!("CARGO_BIN_EXE_test_stdio_server").to_string(),
            args: Vec::new(),
            env: BTreeMap::from([("FIXTURE_ENV".to_string(), "from-test".to_string())]),
            cwd: Some(fixture_cwd.path().display().to_string()),
        },
    };

    let requests = Arc::new(Mutex::new(Vec::new()));
    let process_executor = Arc::new(RecordingExecutor {
        inner: Arc::new(HostProcessExecutor),
        requests: requests.clone(),
    });
    let servers = timeout(
        Duration::from_secs(10),
        connect_and_catalog_mcp_servers_with_options(
            &[config],
            McpConnectOptions {
                process_executor,
                ..Default::default()
            },
        ),
    )
    .await
    .expect("fixture server connect timed out")
    .expect("fixture server should connect");

    assert_eq!(servers.len(), 1);
    let server = &servers[0];
    assert_eq!(server.server_name.as_str(), "fixture");
    assert_eq!(
        server.boundary,
        McpToolBoundary::local_process(McpTransportKind::Stdio)
    );

    assert_eq!(server.catalog.tools.len(), 1);
    let tool = &server.catalog.tools[0];
    assert_eq!(tool.name, types::ToolName::from("inspect_context"));
    assert_eq!(
        tool.origin,
        ToolOrigin::Mcp {
            server_name: "fixture".into()
        }
    );
    assert!(tool.approval.read_only);
    assert!(!tool.approval.mutates_state);
    assert_eq!(tool.approval.idempotent, Some(true));
    assert!(!tool.approval.open_world);
    assert_eq!(
        tool.mcp_boundary,
        Some(McpToolBoundary::local_process(McpTransportKind::Stdio))
    );
    assert!(tool.output_schema.is_some());

    assert_eq!(server.catalog.prompts.len(), 1);
    let prompt_listing = &server.catalog.prompts[0];
    assert_eq!(prompt_listing.name, "draft_brief");
    assert_eq!(prompt_listing.arguments.len(), 1);
    assert!(prompt_listing.arguments[0].required);

    assert_eq!(server.catalog.resources.len(), 1);
    let resource_listing = &server.catalog.resources[0];
    assert_eq!(resource_listing.uri, "fixture://guide");
    assert_eq!(resource_listing.mime_type.as_deref(), Some("text/markdown"));
    assert_eq!(server.catalog.resource_templates.len(), 1);
    assert_eq!(
        server.catalog.resource_templates[0].uri_template,
        "fixture://guide/{section}"
    );

    let tool_result = timeout(
        Duration::from_secs(10),
        server
            .client
            .call_tool("inspect_context", json!({ "subject": "release" })),
    )
    .await
    .expect("fixture tool call timed out")
    .expect("fixture tool call should succeed");
    assert!(!tool_result.is_error);
    assert_eq!(
        tool_result.metadata,
        Some(json!({
            "subject": "release",
            "cwd": expected_cwd.display().to_string(),
            "fixture_env": "from-test"
        }))
    );
    assert!(tool_result.parts.iter().any(|part| matches!(
        part,
        MessagePart::Text { text } if text.contains("\"fixture_env\":\"from-test\"")
    )));
    assert!(tool_result.parts.iter().any(|part| matches!(
        part,
        MessagePart::Resource { uri, text, .. }
            if uri == "fixture://tool-link" && text.as_deref() == Some("linked from tool result")
    )));

    let prompt = timeout(
        Duration::from_secs(10),
        server
            .client
            .get_prompt("draft_brief", json!({ "subject": "release" })),
    )
    .await
    .expect("fixture prompt fetch timed out")
    .expect("fixture prompt fetch should succeed");
    assert_eq!(prompt.name, "draft_brief");
    assert_eq!(prompt.messages.len(), 2);
    assert!(matches!(
        &prompt.messages[0].parts[0],
        MessagePart::Text { text } if text == "Draft a brief for release."
    ));
    assert!(matches!(
        &prompt.messages[1].parts[0],
        MessagePart::Resource { uri, text, .. }
            if uri == "fixture://prompt-resource" && text.as_deref() == Some("context for release")
    ));

    let resource = timeout(
        Duration::from_secs(10),
        server.client.read_resource("fixture://guide"),
    )
    .await
    .expect("fixture resource fetch timed out")
    .expect("fixture resource fetch should succeed");
    assert_eq!(resource.uri, "fixture://guide");
    assert!(matches!(
        &resource.parts[0],
        MessagePart::Resource { text, mime_type, .. }
            if text.as_deref() == Some("# Fixture Guide\n\nThis is fixture content.")
                && mime_type.as_deref() == Some("text/markdown")
    ));

    let logged = requests.lock().unwrap();
    assert_eq!(logged.len(), 1);
    assert_eq!(
        logged[0].origin,
        tools::ExecutionOrigin::McpStdioServer {
            server_name: "fixture".into()
        }
    );
    assert_eq!(logged[0].program, env!("CARGO_BIN_EXE_test_stdio_server"));
    assert_eq!(logged[0].cwd.as_deref(), Some(fixture_cwd.path()));
    assert_eq!(logged[0].stderr, tools::ProcessStdio::Piped);
}
