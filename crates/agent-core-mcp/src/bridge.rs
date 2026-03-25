use crate::McpClient;
use agent_core_tools::McpToolAdapter;
use agent_core_types::{ToolCallId, ToolResult};
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;

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
                        client.call_tool(&tool_name, arguments).await.map(
                            |mut tool_result: ToolResult| {
                                tool_result.id = call_id;
                                tool_result
                            },
                        )
                    })
                }),
            )
        })
        .collect();
    Ok(adapters)
}
