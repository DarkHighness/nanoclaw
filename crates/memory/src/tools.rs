use crate::{
    MemoryBackend, MemoryForgetRequest, MemoryGetRequest, MemoryListRequest, MemoryPromoteRequest,
    MemoryRecordMode, MemoryRecordRequest, MemoryScope, MemorySearchRequest, MemoryStatus,
    MemoryType,
};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use tools::ToolExecutionContext;
use tools::annotations::{builtin_tool_spec, tool_approval_profile};
use tools::registry::Tool;
use types::{ToolCallId, ToolOutputMode, ToolResult, ToolSpec};

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct MemorySearchToolInput {
    pub query: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub path_prefix: Option<String>,
    #[serde(default)]
    pub scopes: Option<Vec<MemoryScope>>,
    #[serde(default)]
    pub types: Option<Vec<MemoryType>>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub agent_session_id: Option<String>,
    #[serde(default)]
    pub agent_name: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub include_stale: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct MemoryGetToolInput {
    pub path: String,
    #[serde(default)]
    pub start_line: Option<usize>,
    #[serde(default)]
    pub line_count: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct MemoryListToolInput {
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub path_prefix: Option<String>,
    #[serde(default)]
    pub scopes: Option<Vec<MemoryScope>>,
    #[serde(default)]
    pub types: Option<Vec<MemoryType>>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub agent_session_id: Option<String>,
    #[serde(default)]
    pub agent_name: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub include_stale: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct MemoryRecordToolInput {
    pub scope: MemoryScope,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub mode: MemoryRecordMode,
    #[serde(default)]
    pub memory_type: Option<MemoryType>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub layer: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub agent_session_id: Option<String>,
    #[serde(default)]
    pub agent_name: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct MemoryPromoteToolInput {
    pub source_path: String,
    pub target_scope: MemoryScope,
    pub title: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub memory_type: Option<MemoryType>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub layer: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct MemoryForgetToolInput {
    pub path: String,
    pub status: MemoryStatus,
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

pub struct MemoryListTool {
    backend: Arc<dyn MemoryBackend>,
}

impl MemoryListTool {
    #[must_use]
    pub fn new(backend: Arc<dyn MemoryBackend>) -> Self {
        Self { backend }
    }
}

pub struct MemoryRecordTool {
    backend: Arc<dyn MemoryBackend>,
}

impl MemoryRecordTool {
    #[must_use]
    pub fn new(backend: Arc<dyn MemoryBackend>) -> Self {
        Self { backend }
    }
}

pub struct MemoryPromoteTool {
    backend: Arc<dyn MemoryBackend>,
}

impl MemoryPromoteTool {
    #[must_use]
    pub fn new(backend: Arc<dyn MemoryBackend>) -> Self {
        Self { backend }
    }
}

pub struct MemoryForgetTool {
    backend: Arc<dyn MemoryBackend>,
}

impl MemoryForgetTool {
    #[must_use]
    pub fn new(backend: Arc<dyn MemoryBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl Tool for MemorySearchTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "memory_search",
            "Search workspace memory when prior decisions, user preferences, previous sessions, incidents, or other non-code context may matter for the current task.",
            serde_json::to_value(schema_for!(MemorySearchToolInput)).expect("memory_search schema"),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, true, false),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> tools::Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: MemorySearchToolInput = serde_json::from_value(arguments)?;
        let response = self
            .backend
            .search(MemorySearchRequest {
                query: input.query,
                limit: input.limit,
                path_prefix: input.path_prefix,
                scopes: input.scopes,
                types: input.types,
                tags: normalize_list(input.tags),
                session_id: normalize_id(input.session_id).or_else(|| ctx.session_id.clone()),
                agent_session_id: normalize_session_id(input.agent_session_id)
                    .or_else(|| ctx.agent_session_id.clone()),
                agent_name: normalize_string(input.agent_name)
                    .or_else(|| inherited_agent_name(ctx)),
                task_id: normalize_string(input.task_id).or_else(|| inherited_task_id(ctx)),
                include_stale: input.include_stale,
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
                "{}. {}:{}-{} score={:.3} scope={} type={} status={}",
                index + 1,
                hit.path,
                hit.start_line,
                hit.end_line,
                hit.score,
                hit.document_metadata.scope.as_str(),
                display_memory_type(hit.document_metadata.memory_type),
                hit.document_metadata.status.as_str()
            ));
            maybe_push_description_line(&mut lines, hit.document_metadata.description.as_deref());
            lines.push(hit.snippet.clone());
        }
        let structured_output = json!({
            "backend": response.backend,
            "hits": response.hits,
            "metadata": response.metadata,
        });
        let mut result = ToolResult::text(call_id, "memory_search", lines.join("\n"))
            .with_call_id(external_call_id)
            .with_structured_content(structured_output.clone());
        result.metadata = Some(structured_output);
        Ok(result)
    }
}

#[async_trait]
impl Tool for MemoryGetTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "memory_get",
            "Read a specific memory note after memory_search or memory_list so you can verify the exact source before relying on it.",
            serde_json::to_value(schema_for!(MemoryGetToolInput)).expect("memory_get schema"),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, true, false),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        _ctx: &ToolExecutionContext,
    ) -> tools::Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
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
            "[memory_get path={} title={} scope={} type={} lines={}-{} / {} snapshot={}]\n{}",
            document.path,
            document.title,
            document.metadata.scope.as_str(),
            display_memory_type(document.metadata.memory_type),
            document.resolved_start_line,
            document.resolved_end_line,
            document.total_lines,
            document.snapshot_id,
            body
        );
        let structured_output = json!({
            "path": document.path,
            "title": document.title,
            "snapshot_id": document.snapshot_id,
            "requested_start_line": document.requested_start_line,
            "resolved_start_line": document.resolved_start_line,
            "resolved_end_line": document.resolved_end_line,
            "total_lines": document.total_lines,
            "metadata": document.metadata,
            "text": document.text,
        });
        let mut result = ToolResult::text(call_id, "memory_get", output)
            .with_call_id(external_call_id)
            .with_structured_content(structured_output.clone());
        result.metadata = Some(structured_output);
        Ok(result)
    }
}

#[async_trait]
impl Tool for MemoryListTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "memory_list",
            "Browse the current memory inventory before choosing which note to open with memory_get.",
            serde_json::to_value(schema_for!(MemoryListToolInput)).expect("memory_list schema"),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, true, false),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> tools::Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: MemoryListToolInput = serde_json::from_value(arguments)?;
        let response = self
            .backend
            .list(MemoryListRequest {
                limit: input.limit,
                path_prefix: input.path_prefix,
                scopes: input.scopes,
                types: input.types,
                tags: normalize_list(input.tags),
                session_id: normalize_id(input.session_id).or_else(|| ctx.session_id.clone()),
                agent_session_id: normalize_session_id(input.agent_session_id)
                    .or_else(|| ctx.agent_session_id.clone()),
                agent_name: normalize_string(input.agent_name)
                    .or_else(|| inherited_agent_name(ctx)),
                task_id: normalize_string(input.task_id).or_else(|| inherited_task_id(ctx)),
                include_stale: input.include_stale,
            })
            .await
            .map_err(|error| tools::ToolError::invalid(error.to_string()))?;
        let mut lines = vec![format!("[memory_list entries={}]", response.entries.len())];
        for (index, entry) in response.entries.iter().enumerate() {
            lines.push(format!(
                "{}. {} scope={} type={} status={} title={}",
                index + 1,
                entry.path,
                entry.metadata.scope.as_str(),
                display_memory_type(entry.metadata.memory_type),
                entry.metadata.status.as_str(),
                entry.title
            ));
            maybe_push_description_line(&mut lines, entry.metadata.description.as_deref());
        }
        let structured_output = json!({
            "entries": response.entries,
            "metadata": response.metadata,
        });
        let mut result = ToolResult::text(call_id, "memory_list", lines.join("\n"))
            .with_call_id(external_call_id)
            .with_structured_content(structured_output.clone());
        result.metadata = Some(structured_output);
        Ok(result)
    }
}

#[async_trait]
impl Tool for MemoryRecordTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "memory_record",
            "Write workspace memory intentionally when the user asks you to remember something, when you need to persist a verified handoff-worthy fact, or when you need to replace the current working session note with updated state under .nanoclaw/memory.",
            serde_json::to_value(schema_for!(MemoryRecordToolInput)).expect("memory_record schema"),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, true, false),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> tools::Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: MemoryRecordToolInput = serde_json::from_value(arguments)?;
        let response = self
            .backend
            .record(MemoryRecordRequest {
                scope: input.scope,
                title: input.title,
                content: input.content,
                mode: input.mode,
                memory_type: input.memory_type,
                description: normalize_string(input.description),
                layer: input.layer,
                tags: input.tags,
                session_id: normalize_id(input.session_id).or_else(|| ctx.session_id.clone()),
                agent_session_id: normalize_session_id(input.agent_session_id)
                    .or_else(|| ctx.agent_session_id.clone()),
                agent_name: normalize_string(input.agent_name)
                    .or_else(|| inherited_agent_name(ctx)),
                task_id: normalize_string(input.task_id).or_else(|| inherited_task_id(ctx)),
            })
            .await
            .map_err(|error| tools::ToolError::invalid(error.to_string()))?;
        mutation_result(call_id, external_call_id, "memory_record", response)
    }
}

#[async_trait]
impl Tool for MemoryPromoteTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "memory_promote",
            "Promote verified working or episodic memory into procedural or semantic memory, preserving or overriding type and description metadata.",
            serde_json::to_value(schema_for!(MemoryPromoteToolInput))
                .expect("memory_promote schema"),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, true, false),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        _ctx: &ToolExecutionContext,
    ) -> tools::Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: MemoryPromoteToolInput = serde_json::from_value(arguments)?;
        let response = self
            .backend
            .promote(MemoryPromoteRequest {
                source_path: input.source_path,
                target_scope: input.target_scope,
                title: input.title,
                content: input.content,
                memory_type: input.memory_type,
                description: normalize_string(input.description),
                layer: input.layer,
                tags: input.tags,
            })
            .await
            .map_err(|error| tools::ToolError::invalid(error.to_string()))?;
        mutation_result(call_id, external_call_id, "memory_promote", response)
    }
}

#[async_trait]
impl Tool for MemoryForgetTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "memory_forget",
            "Mark memory as stale, superseded, or archived without deleting the Markdown source.",
            serde_json::to_value(schema_for!(MemoryForgetToolInput)).expect("memory_forget schema"),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, true, false),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        _ctx: &ToolExecutionContext,
    ) -> tools::Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: MemoryForgetToolInput = serde_json::from_value(arguments)?;
        let response = self
            .backend
            .forget(MemoryForgetRequest {
                path: input.path,
                status: input.status,
            })
            .await
            .map_err(|error| tools::ToolError::invalid(error.to_string()))?;
        mutation_result(call_id, external_call_id, "memory_forget", response)
    }
}

fn mutation_result(
    call_id: ToolCallId,
    external_call_id: types::CallId,
    tool_name: &str,
    response: crate::MemoryMutationResponse,
) -> tools::Result<ToolResult> {
    let text = format!(
        "[{} action={} path={} scope={} type={} status={} snapshot={}]",
        tool_name,
        response.action,
        response.path,
        response.metadata.scope.as_str(),
        display_memory_type(response.metadata.memory_type),
        response.metadata.status.as_str(),
        response.snapshot_id
    );
    let structured_output = json!({
        "action": response.action,
        "path": response.path,
        "snapshot_id": response.snapshot_id,
        "metadata": response.metadata,
    });
    let mut result = ToolResult::text(call_id, tool_name, text)
        .with_call_id(external_call_id)
        .with_structured_content(structured_output.clone());
    result.metadata = Some(structured_output);
    Ok(result)
}

fn normalize_id(value: Option<String>) -> Option<types::SessionId> {
    normalize_string(value).map(types::SessionId::from)
}

fn normalize_session_id(value: Option<String>) -> Option<types::AgentSessionId> {
    normalize_string(value).map(types::AgentSessionId::from)
}

fn normalize_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn normalize_list(value: Option<Vec<String>>) -> Option<Vec<String>> {
    value.and_then(|items| {
        let mut normalized = items
            .into_iter()
            .filter_map(|item| {
                let trimmed = item.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            })
            .collect::<Vec<_>>();
        normalized.sort();
        normalized.dedup();
        (!normalized.is_empty()).then_some(normalized)
    })
}

fn display_memory_type(memory_type: Option<MemoryType>) -> &'static str {
    memory_type.map(MemoryType::as_str).unwrap_or("-")
}

fn maybe_push_description_line(lines: &mut Vec<String>, description: Option<&str>) {
    if let Some(description) = description.map(str::trim).filter(|value| !value.is_empty()) {
        lines.push(format!("description: {description}"));
    }
}

fn inherited_agent_name(ctx: &ToolExecutionContext) -> Option<String> {
    normalize_string(ctx.agent_name.clone()).or_else(|| {
        ctx.agent_id
            .as_ref()
            .map(ToString::to_string)
            .and_then(|value| normalize_string(Some(value)))
    })
}

fn inherited_task_id(ctx: &ToolExecutionContext) -> Option<String> {
    normalize_string(ctx.task_id.clone())
}

#[cfg(test)]
mod tests {
    use super::{MemoryGetTool, MemoryListTool, MemoryRecordTool, MemorySearchTool};
    use crate::{MemoryCoreBackend, MemoryCoreConfig, MemoryScope};
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::fs;
    use tools::{Tool, ToolExecutionContext};
    use types::{AgentId, AgentSessionId, SessionId, ToolCallId};

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
    async fn memory_search_tool_reports_scope_and_hits() {
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
        assert!(result.text_content().contains("scope=semantic"));
    }

    #[tokio::test]
    async fn memory_record_tool_uses_runtime_scope_defaults() {
        let dir = tempdir().unwrap();
        let backend = Arc::new(MemoryCoreBackend::new(
            dir.path().to_path_buf(),
            MemoryCoreConfig::default(),
        ));
        let tool = MemoryRecordTool::new(backend);
        let ctx = ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            session_id: Some(SessionId::from("session_1")),
            agent_session_id: Some(AgentSessionId::from("agent_session_1")),
            ..ToolExecutionContext::default()
        };
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "scope":"working",
                    "title":"Debug note",
                    "content":"Keep this in session scratchpad"
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(result.text_content().contains("scope=working"));
        let recorded = fs::read_to_string(
            dir.path()
                .join(".nanoclaw/memory/working/agent-sessions/agent_session_1.md"),
        )
        .await
        .unwrap();
        assert!(recorded.contains("Keep this in session scratchpad"));
        assert!(recorded.contains("session_id: session_1"));
    }

    #[tokio::test]
    async fn memory_record_tool_uses_agent_and_task_scope_defaults() {
        let dir = tempdir().unwrap();
        let backend = Arc::new(MemoryCoreBackend::new(
            dir.path().to_path_buf(),
            MemoryCoreConfig::default(),
        ));
        let tool = MemoryRecordTool::new(backend);
        let ctx = ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            session_id: Some(SessionId::from("session_1")),
            agent_session_id: Some(AgentSessionId::from("agent_session_1")),
            agent_id: Some(AgentId::from("agent_child")),
            agent_name: Some("reviewer".to_string()),
            task_id: Some("task_17".to_string()),
            ..ToolExecutionContext::default()
        };
        tool.execute(
            ToolCallId::new(),
            json!({
                "scope":"working",
                "title":"Task note",
                "content":"Track the current child task"
            }),
            &ctx,
        )
        .await
        .unwrap();

        let recorded =
            fs::read_to_string(dir.path().join(".nanoclaw/memory/working/tasks/task-17.md"))
                .await
                .unwrap();
        assert!(recorded.contains("agent_name: reviewer"));
        assert!(recorded.contains("task_id: task_17"));
    }

    #[tokio::test]
    async fn memory_record_tool_replace_mode_keeps_only_latest_body() {
        let dir = tempdir().unwrap();
        let backend = Arc::new(MemoryCoreBackend::new(
            dir.path().to_path_buf(),
            MemoryCoreConfig::default(),
        ));
        let tool = MemoryRecordTool::new(backend);
        let ctx = ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            session_id: Some(SessionId::from("session_1")),
            agent_session_id: Some(AgentSessionId::from("agent_session_1")),
            ..ToolExecutionContext::default()
        };

        tool.execute(
            ToolCallId::new(),
            json!({
                "scope":"working",
                "title":"Session continuation snapshot",
                "content":"Current State\n\nfirst snapshot",
                "mode":"replace"
            }),
            &ctx,
        )
        .await
        .unwrap();
        tool.execute(
            ToolCallId::new(),
            json!({
                "scope":"working",
                "title":"Session continuation snapshot",
                "content":"Current State\n\nsecond snapshot",
                "mode":"replace"
            }),
            &ctx,
        )
        .await
        .unwrap();

        let recorded = fs::read_to_string(
            dir.path()
                .join(".nanoclaw/memory/working/agent-sessions/agent_session_1.md"),
        )
        .await
        .unwrap();
        assert!(recorded.contains("second snapshot"));
        assert!(!recorded.contains("first snapshot"));
    }

    #[tokio::test]
    async fn memory_record_tool_defaults_episodic_scope_to_daily_log() {
        let dir = tempdir().unwrap();
        let backend = Arc::new(MemoryCoreBackend::new(
            dir.path().to_path_buf(),
            MemoryCoreConfig::default(),
        ));
        let tool = MemoryRecordTool::new(backend);
        let ctx = ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            session_id: Some(SessionId::from("session_1")),
            agent_session_id: Some(AgentSessionId::from("agent_session_1")),
            ..ToolExecutionContext::default()
        };
        let today = time::OffsetDateTime::now_utc().date();
        let month = u8::from(today.month());

        tool.execute(
            ToolCallId::new(),
            json!({
                "scope":"episodic",
                "title":"Daily capture",
                "content":"- user prefers canary deploys"
            }),
            &ctx,
        )
        .await
        .unwrap();

        let recorded = fs::read_to_string(dir.path().join(format!(
            ".nanoclaw/memory/episodic/logs/{:04}/{:02}/{}.md",
            today.year(),
            month,
            today
        )))
        .await
        .unwrap();
        assert!(recorded.contains("scope: episodic"));
        assert!(recorded.contains("layer: daily-log"));
        assert!(recorded.contains("user prefers canary deploys"));
    }

    #[tokio::test]
    async fn memory_search_tool_inherits_agent_and_task_scope_filters() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".nanoclaw/memory/episodic/tasks"))
            .await
            .unwrap();
        fs::create_dir_all(dir.path().join(".nanoclaw/memory/episodic/subagents"))
            .await
            .unwrap();
        fs::write(
            dir.path()
                .join(".nanoclaw/memory/episodic/tasks/session-1--agent-session-1--task-17.md"),
            "---\nscope: episodic\nlayer: runtime-task\nsession_id: session_1\nagent_session_id: agent_session_1\nagent_name: reviewer\ntask_id: task_17\nstatus: ready\n---\n# Task task_17\n\nchecked ownership",
        )
        .await
        .unwrap();
        fs::write(
            dir.path()
                .join(".nanoclaw/memory/episodic/tasks/session-1--agent-session-1--task-99.md"),
            "---\nscope: episodic\nlayer: runtime-task\nsession_id: session_1\nagent_session_id: agent_session_1\nagent_name: reviewer\ntask_id: task_99\nstatus: ready\n---\n# Task task_99\n\nchecked ownership elsewhere",
        )
        .await
        .unwrap();

        let backend = Arc::new(MemoryCoreBackend::new(
            dir.path().to_path_buf(),
            MemoryCoreConfig::default(),
        ));
        let tool = MemorySearchTool::new(backend);
        let ctx = ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            session_id: Some(SessionId::from("session_1")),
            agent_session_id: Some(AgentSessionId::from("agent_session_1")),
            agent_id: Some(AgentId::from("agent_child")),
            agent_name: Some("reviewer".to_string()),
            task_id: Some("task_17".to_string()),
            ..ToolExecutionContext::default()
        };
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({"query":"checked ownership"}),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.text_content().contains("hits=1"));
        let structured = result.structured_content.unwrap();
        assert_eq!(
            structured["hits"][0]["document_metadata"]["task_id"],
            "task_17"
        );
        assert_eq!(
            structured["hits"][0]["document_metadata"]["agent_name"],
            "reviewer"
        );
    }

    #[tokio::test]
    async fn memory_list_tool_hides_non_ready_entries_by_default() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".nanoclaw/memory/semantic"))
            .await
            .unwrap();
        fs::write(
            dir.path().join(".nanoclaw/memory/semantic/ready.md"),
            "---\nscope: semantic\nlayer: rule\nstatus: ready\n---\n# Ready\n\nkeep",
        )
        .await
        .unwrap();
        fs::write(
            dir.path().join(".nanoclaw/memory/semantic/archived.md"),
            "---\nscope: semantic\nlayer: rule\nstatus: archived\n---\n# Archived\n\nhide by default",
        )
        .await
        .unwrap();

        let backend = Arc::new(MemoryCoreBackend::new(
            dir.path().to_path_buf(),
            MemoryCoreConfig::default(),
        ));
        let tool = MemoryListTool::new(backend);

        let default_result = tool
            .execute(
                ToolCallId::new(),
                json!({"path_prefix":".nanoclaw/memory/semantic/"}),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert!(
            default_result
                .text_content()
                .contains("[memory_list entries=1]")
        );
        assert!(default_result.text_content().contains("status=ready"));
        assert!(!default_result.text_content().contains("status=archived"));

        let full_result = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "path_prefix":".nanoclaw/memory/semantic/",
                    "include_stale": true
                }),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert!(
            full_result
                .text_content()
                .contains("[memory_list entries=2]")
        );
        assert!(full_result.text_content().contains("status=archived"));
    }

    #[test]
    fn memory_scope_schema_serializes_as_kebab_case() {
        assert_eq!(
            serde_json::to_string(&MemoryScope::Working).unwrap(),
            "\"working\""
        );
        assert_eq!(
            serde_json::from_str::<MemoryScope>("\"coordination\"").unwrap(),
            MemoryScope::Working
        );
    }
}
