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
        _ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let mut result = (self.handler)(call_id.clone(), arguments).await?;
        let upstream_call_id = result.call_id.clone();
        let upstream_tool_name = result.tool_name.clone();

        result.id = call_id.clone();
        // Runtime transcripts and compaction index tool results by the local call id.
        // Keep upstream ids in metadata so audits can still correlate remote traces.
        result.call_id = call_id.0.clone().into();
        result.tool_name = self.spec.name.clone();
        result.metadata = Some(augment_metadata(
            result.metadata,
            upstream_call_id.as_str(),
            &upstream_tool_name,
            &call_id.0,
            &self.spec.name,
        ));
        Ok(result)
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
    use types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};

    fn test_spec() -> ToolSpec {
        ToolSpec {
            name: "remote_echo".to_string(),
            description: "test".to_string(),
            input_schema: json!({"type": "object"}),
            output_mode: ToolOutputMode::Text,
            origin: ToolOrigin::Mcp {
                server_name: "test-server".to_string(),
            },
            annotations: Default::default(),
        }
    }

    #[tokio::test]
    async fn mcp_adapter_normalizes_call_and_tool_identity() {
        let adapter = McpToolAdapter::new(
            test_spec(),
            Arc::new(|_call_id, _arguments| {
                Box::pin(async move {
                    Ok(ToolResult {
                        id: ToolCallId("upstream-id".to_string()),
                        call_id: "remote-call-1".into(),
                        tool_name: "other-name".to_string(),
                        parts: vec![MessagePart::text("ok")],
                        metadata: Some(json!({"raw": true})),
                        is_error: false,
                    })
                })
            }),
        );

        let call_id = ToolCallId("local-call-1".to_string());
        let result = adapter
            .execute(
                call_id.clone(),
                json!({"x": 1}),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert_eq!(result.id.0, call_id.0);
        assert_eq!(result.call_id.as_str(), "local-call-1");
        assert_eq!(result.tool_name, "remote_echo");
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
                        tool_name: "remote_echo".to_string(),
                        parts: vec![MessagePart::text("ok")],
                        metadata: Some(json!("plain")),
                        is_error: false,
                    })
                })
            }),
        );

        let result = adapter
            .execute(
                ToolCallId("local-call-2".to_string()),
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
}
