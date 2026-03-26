use crate::{MemoryBackend, MemoryGetRequest, MemorySearchRequest};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use tools::ToolExecutionContext;
use tools::annotations::mcp_tool_annotations;
use tools::registry::Tool;
use types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct MemorySearchToolInput {
    pub query: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub path_prefix: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct MemoryGetToolInput {
    pub path: String,
    #[serde(default)]
    pub start_line: Option<usize>,
    #[serde(default)]
    pub line_count: Option<usize>,
}

pub struct MemorySearchTool {
    backend: Arc<dyn MemoryBackend>,
}

impl MemorySearchTool {
    #[must_use]
    pub fn new(backend: Arc<dyn MemoryBackend>) -> Self {
        Self { backend }
    }
}

pub struct MemoryGetTool {
    backend: Arc<dyn MemoryBackend>,
}

impl MemoryGetTool {
    #[must_use]
    pub fn new(backend: Arc<dyn MemoryBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl Tool for MemorySearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "memory_search".to_string(),
            description: "Search curated workspace memory Markdown files and return bounded snippets with file and line citations.".to_string(),
            input_schema: serde_json::to_value(schema_for!(MemorySearchToolInput))
                .expect("memory_search schema"),
            output_mode: ToolOutputMode::Text,
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("Memory Search", true, false, true, false),
        }
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        _ctx: &ToolExecutionContext,
    ) -> tools::Result<ToolResult> {
        let input: MemorySearchToolInput = serde_json::from_value(arguments)?;
        let response = self
            .backend
            .search(MemorySearchRequest {
                query: input.query,
                limit: input.limit,
                path_prefix: input.path_prefix,
            })
            .await
            .map_err(|error| tools::ToolError::invalid(error.to_string()))?;
        let mut lines = vec![format!(
            "[memory_search backend={} hits={}]",
            response.backend,
            response.hits.len()
        )];
        for (index, hit) in response.hits.iter().enumerate() {
            lines.push(format!(
                "{}. {}:{}-{} score={:.3}",
                index + 1,
                hit.path,
                hit.start_line,
                hit.end_line,
                hit.score
            ));
            lines.push(hit.snippet.clone());
        }
        Ok(ToolResult {
            id: call_id.clone(),
            call_id: types::CallId::from(&call_id),
            tool_name: "memory_search".to_string(),
            parts: vec![MessagePart::text(lines.join("\n"))],
            metadata: Some(json!({
                "backend": response.backend,
                "hits": response.hits,
                "metadata": response.metadata,
            })),
            is_error: false,
        })
    }
}

#[async_trait]
impl Tool for MemoryGetTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "memory_get".to_string(),
            description: "Read a specific memory file from the configured memory corpus, optionally starting at a line offset.".to_string(),
            input_schema: serde_json::to_value(schema_for!(MemoryGetToolInput))
                .expect("memory_get schema"),
            output_mode: ToolOutputMode::Text,
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("Memory Get", true, false, true, false),
        }
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        _ctx: &ToolExecutionContext,
    ) -> tools::Result<ToolResult> {
        let input: MemoryGetToolInput = serde_json::from_value(arguments)?;
        let document = self
            .backend
            .get(MemoryGetRequest {
                path: input.path,
                start_line: input.start_line,
                line_count: input.line_count,
            })
            .await
            .map_err(|error| tools::ToolError::invalid(error.to_string()))?;
        let body = if document.text.is_empty() {
            "[Memory file is empty]".to_string()
        } else {
            document.text.clone()
        };
        let output = format!(
            "[memory_get path={} lines={}-{} / {} snapshot={}]\n{}",
            document.path,
            document.resolved_start_line,
            document.resolved_end_line,
            document.total_lines,
            document.snapshot_id,
            body
        );
        Ok(ToolResult {
            id: call_id.clone(),
            call_id: types::CallId::from(&call_id),
            tool_name: "memory_get".to_string(),
            parts: vec![MessagePart::text(output)],
            metadata: Some(json!({
                "path": document.path,
                "snapshot_id": document.snapshot_id,
                "requested_start_line": document.requested_start_line,
                "resolved_start_line": document.resolved_start_line,
                "resolved_end_line": document.resolved_end_line,
                "total_lines": document.total_lines,
            })),
            is_error: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{MemoryGetTool, MemorySearchTool};
    use crate::{MemoryCoreBackend, MemoryCoreConfig};
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::fs;
    use tools::{Tool, ToolExecutionContext};
    use types::ToolCallId;

    #[tokio::test]
    async fn memory_get_tool_formats_numbered_lines() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("MEMORY.md"), "alpha\nbeta\ngamma")
            .await
            .unwrap();
        let backend = Arc::new(MemoryCoreBackend::new(
            dir.path().to_path_buf(),
            MemoryCoreConfig::default(),
        ));
        let tool = MemoryGetTool::new(backend);
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({"path":"MEMORY.md","start_line":2,"line_count":1}),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert!(result.text_content().contains("2 | beta"));
    }

    #[tokio::test]
    async fn memory_search_tool_reports_hits() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("MEMORY.md"), "redis sentinel")
            .await
            .unwrap();
        let backend = Arc::new(MemoryCoreBackend::new(
            dir.path().to_path_buf(),
            MemoryCoreConfig::default(),
        ));
        let tool = MemorySearchTool::new(backend);
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({"query":"redis"}),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert!(
            result
                .text_content()
                .contains("[memory_search backend=memory-core hits=1]")
        );
    }
}
