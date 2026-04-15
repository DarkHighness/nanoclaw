use crate::{Result, Tool, ToolExecutionContext};
use async_trait::async_trait;
use serde_json::{Map, Value, json};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use types::{ToolCallId, ToolResult, ToolSpec};

type McpToolFuture = Pin<Box<dyn Future<Output = Result<ToolResult>> + Send>>;

#[derive(Clone)]
pub struct McpToolAdapter {
    spec: ToolSpec,
    handler: Arc<dyn Fn(ToolCallId, Value) -> McpToolFuture + Send + Sync>,
}

impl McpToolAdapter {
    #[must_use]
    pub fn new(
        spec: ToolSpec,
        handler: Arc<dyn Fn(ToolCallId, Value) -> McpToolFuture + Send + Sync>,
    ) -> Self {
        Self { spec, handler }
    }

    #[must_use]
    pub fn with_spec(&self, spec: ToolSpec) -> Self {
        Self {
            spec,
            handler: self.handler.clone(),
        }
    }
}

#[async_trait]
impl Tool for McpToolAdapter {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        if let Some(server_name) = mcp_server_name_for_spec(&self.spec) {
            ctx.assert_mcp_server_allowed(server_name)?;
        }
        let mut result = (self.handler)(call_id.clone(), arguments).await?;
        let upstream_call_id = result.call_id.clone();
        let upstream_tool_name = result.tool_name.clone();

        result.id = call_id.clone();
        // Runtime transcripts and compaction index tool results by the local call id.
        // Keep upstream ids in metadata so audits can still correlate remote traces.
        result.call_id = (&call_id).into();
        result.tool_name = self.spec.name.clone();
        result.metadata = Some(augment_metadata(
            result.metadata,
            upstream_call_id.as_str(),
            upstream_tool_name.as_str(),
            call_id.as_str(),
            self.spec.name.as_str(),
        ));
        Ok(result)
    }
}

fn mcp_server_name_for_spec(spec: &ToolSpec) -> Option<&types::McpServerName> {
    match &spec.source {
        types::ToolSource::McpTool { server_name }
        | types::ToolSource::McpResource { server_name }
            if server_name.as_str() != "*" =>
        {
            Some(server_name)
        }
        _ => None,
    }
}

fn augment_metadata(
    metadata: Option<Value>,
    upstream_call_id: &str,
    upstream_tool_name: &str,
    normalized_call_id: &str,
    normalized_tool_name: &str,
) -> Value {
    let mut object = match metadata {
        Some(Value::Object(object)) => object,
        Some(other) => {
            let mut object = Map::new();
            object.insert("upstream_metadata".to_string(), other);
            object
        }
        None => Map::new(),
    };
    object.insert(
        "mcp_adapter".to_string(),
        json!({
            "upstream_call_id": upstream_call_id,
            "upstream_tool_name": upstream_tool_name,
            "normalized_call_id": normalized_call_id,
            "normalized_tool_name": normalized_tool_name,
        }),
    );
    Value::Object(object)
}

#[cfg(test)]
mod tests {
    use super::McpToolAdapter;
    use crate::{Tool, ToolExecutionContext};
    use serde_json::json;
    use std::sync::Arc;
    use types::{
        MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSource, ToolSpec,
    };

    fn test_spec() -> ToolSpec {
        ToolSpec::function(
            "remote_echo",
            "test",
            json!({"type": "object"}),
            ToolOutputMode::Text,
            ToolOrigin::Mcp {
                server_name: "test-server".into(),
            },
            ToolSource::McpTool {
                server_name: "test-server".into(),
            },
        )
    }

    #[tokio::test]
    async fn mcp_adapter_normalizes_call_and_tool_identity() {
        let adapter = McpToolAdapter::new(
            test_spec(),
            Arc::new(|_call_id, _arguments| {
                Box::pin(async move {
                    Ok(ToolResult {
                        id: ToolCallId::from("upstream-id"),
                        call_id: "remote-call-1".into(),
                        tool_name: "other-name".into(),
                        parts: vec![MessagePart::text("ok")],
                        attachments: Vec::new(),
                        structured_content: None,
                        continuation: None,
                        metadata: Some(json!({"raw": true})),
                        is_error: false,
                    })
                })
            }),
        );

        let call_id = ToolCallId::from("local-call-1");
        let result = adapter
            .execute(
                call_id.clone(),
                json!({"x": 1}),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert_eq!(result.id, call_id);
        assert_eq!(result.call_id.as_str(), "local-call-1");
        assert_eq!(result.tool_name, types::ToolName::from("remote_echo"));
        assert_eq!(
            result.metadata.unwrap()["mcp_adapter"]["upstream_call_id"],
            "remote-call-1"
        );
    }

    #[tokio::test]
    async fn mcp_adapter_preserves_non_object_metadata() {
        let adapter = McpToolAdapter::new(
            test_spec(),
            Arc::new(|_call_id, _arguments| {
                Box::pin(async move {
                    Ok(ToolResult {
                        id: ToolCallId::new(),
                        call_id: "remote-call-2".into(),
                        tool_name: "remote_echo".into(),
                        parts: vec![MessagePart::text("ok")],
                        attachments: Vec::new(),
                        structured_content: None,
                        continuation: None,
                        metadata: Some(json!("plain")),
                        is_error: false,
                    })
                })
            }),
        );

        let result = adapter
            .execute(
                ToolCallId::from("local-call-2"),
                json!({}),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let metadata = result.metadata.unwrap();
        assert_eq!(metadata["upstream_metadata"], "plain");
        assert_eq!(
            metadata["mcp_adapter"]["normalized_tool_name"],
            "remote_echo"
        );
    }

    #[test]
    fn mcp_adapter_can_be_rebound_to_a_new_tool_spec() {
        let adapter = McpToolAdapter::new(
            test_spec(),
            Arc::new(|_, _| Box::pin(async move { panic!("handler should not run in spec test") })),
        );
        let rebound = adapter.with_spec(ToolSpec::function(
            "playwright_browser_snapshot",
            "test",
            json!({"type": "object"}),
            ToolOutputMode::Text,
            ToolOrigin::Mcp {
                server_name: "playwright".into(),
            },
            ToolSource::McpTool {
                server_name: "playwright".into(),
            },
        ));
        assert_eq!(rebound.spec().name.as_str(), "playwright_browser_snapshot");
    }
}
