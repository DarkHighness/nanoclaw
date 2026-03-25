use crate::ToolExecutionContext;
use crate::annotations::mcp_tool_annotations;
use crate::registry::Tool;
use crate::{Result, ToolError};
use agent_core_types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};
use async_trait::async_trait;
use regex::Regex;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::OnceLock;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct TaskToolInput {
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub steer: Option<String>,
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
pub struct SubagentRequest {
    pub prompt: String,
    pub agent: Option<String>,
    pub steer: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
pub struct SubagentResult {
    pub run_id: String,
    pub session_id: String,
    pub agent_name: String,
    pub assistant_text: String,
    pub allowed_tools: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct StructuredTaskPayload {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    artifacts: Option<Vec<StructuredTaskArtifact>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StructuredTaskArtifact {
    #[serde(default)]
    kind: Option<String>,
    uri: String,
    #[serde(default)]
    label: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct TaskArtifact {
    kind: String,
    uri: String,
    label: Option<String>,
}

#[async_trait]
pub trait SubagentExecutor: Send + Sync {
    async fn run(&self, request: SubagentRequest) -> Result<SubagentResult>;
}

#[derive(Clone)]
pub struct TaskTool {
    executor: Arc<dyn SubagentExecutor>,
}

impl TaskTool {
    #[must_use]
    pub fn new(executor: Arc<dyn SubagentExecutor>) -> Self {
        Self { executor }
    }
}

fn resolve_prompt(input: &TaskToolInput) -> Result<String> {
    input
        .prompt
        .clone()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ToolError::invalid("task requires prompt"))
}

fn resolve_agent(input: &TaskToolInput) -> Option<String> {
    input
        .agent
        .clone()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[async_trait]
impl Tool for TaskTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "task".to_string(),
            description: "Delegate a scoped prompt to a subagent and return its summary output plus run identifiers."
                .to_string(),
            input_schema: serde_json::to_value(schema_for!(TaskToolInput)).expect("task schema"),
            output_mode: ToolOutputMode::Text,
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("Run Subagent Task", false, false, false, false),
        }
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        _ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = call_id.0.clone();
        let input: TaskToolInput = serde_json::from_value(arguments)?;
        let request = SubagentRequest {
            prompt: resolve_prompt(&input)?,
            agent: resolve_agent(&input),
            steer: input.steer,
            allowed_tools: input.allowed_tools,
        };
        if request.prompt.is_empty() {
            return Err(ToolError::invalid("task prompt must not be empty"));
        }

        let output = self.executor.run(request).await?;
        let normalized = normalize_subagent_text(&output.assistant_text);
        let status = normalized.status.unwrap_or_else(|| {
            if normalized.text.trim().is_empty() {
                "completed_without_output".to_string()
            } else {
                "completed".to_string()
            }
        });
        let summary_line = normalized
            .summary
            .unwrap_or_else(|| summarize_output(&normalized.text));
        let rendered_text = format!(
            "[task agent={} run_id={} session_id={} status={}]\nallowed_tools> {}\nartifacts> {}\nsummary> {}\n\n{}",
            output.agent_name,
            output.run_id,
            output.session_id,
            status,
            if output.allowed_tools.is_empty() {
                "none".to_string()
            } else {
                output.allowed_tools.join(", ")
            },
            normalized.artifacts.len(),
            summary_line,
            if normalized.text.trim().is_empty() {
                "[Subagent completed without textual output]".to_string()
            } else {
                normalized.text.clone()
            }
        );
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "task".to_string(),
            parts: vec![MessagePart::text(rendered_text)],
            metadata: Some(serde_json::json!({
                "run_id": output.run_id,
                "session_id": output.session_id,
                "agent_name": output.agent_name,
                "allowed_tools": output.allowed_tools,
                "status": status,
                "summary": summary_line,
                "artifacts": normalized.artifacts,
                "text": normalized.text,
                "used_structured_payload": normalized.used_structured_payload,
            })),
            is_error: false,
        })
    }
}

#[derive(Clone, Debug)]
struct NormalizedTaskText {
    status: Option<String>,
    summary: Option<String>,
    text: String,
    artifacts: Vec<TaskArtifact>,
    used_structured_payload: bool,
}

fn normalize_subagent_text(text: &str) -> NormalizedTaskText {
    if let Ok(payload) = serde_json::from_str::<StructuredTaskPayload>(text.trim()) {
        let parsed_text = payload.text.unwrap_or_else(|| text.to_string());
        let artifacts = payload
            .artifacts
            .unwrap_or_default()
            .into_iter()
            .filter_map(|artifact| normalize_artifact(artifact.kind, artifact.uri, artifact.label))
            .collect::<Vec<_>>();
        return NormalizedTaskText {
            status: payload.status.map(normalize_status),
            summary: payload.summary.map(|value| value.trim().to_string()),
            text: parsed_text,
            artifacts: if artifacts.is_empty() {
                extract_artifacts(text)
            } else {
                artifacts
            },
            used_structured_payload: true,
        };
    }

    NormalizedTaskText {
        status: None,
        summary: None,
        text: text.to_string(),
        artifacts: extract_artifacts(text),
        used_structured_payload: false,
    }
}

fn summarize_output(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "none".to_string();
    }
    let mut summary = trimmed.lines().next().unwrap_or(trimmed).trim().to_string();
    if summary.chars().count() > 160 {
        summary = summary.chars().take(160).collect::<String>();
        summary.push_str("...");
    }
    summary
}

fn normalize_status(status: String) -> String {
    match status.trim().to_ascii_lowercase().as_str() {
        "completed" | "done" | "success" => "completed".to_string(),
        "needs_follow_up" | "follow_up" | "partial" => "needs_follow_up".to_string(),
        "blocked" => "blocked".to_string(),
        "failed" | "error" => "failed".to_string(),
        other => other.to_string(),
    }
}

fn normalize_artifact(
    kind: Option<String>,
    uri: String,
    label: Option<String>,
) -> Option<TaskArtifact> {
    let uri = uri.trim().to_string();
    if uri.is_empty() {
        return None;
    }
    let kind = kind
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| infer_artifact_kind(&uri).to_string());
    Some(TaskArtifact {
        kind,
        uri,
        label: label.map(|value| value.trim().to_string()),
    })
}

fn extract_artifacts(text: &str) -> Vec<TaskArtifact> {
    static MARKDOWN_LINK_RE: OnceLock<Regex> = OnceLock::new();
    static ABS_PATH_RE: OnceLock<Regex> = OnceLock::new();

    let mut seen = BTreeSet::new();
    let mut artifacts = Vec::new();

    let markdown_link_re = MARKDOWN_LINK_RE
        .get_or_init(|| Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").expect("markdown link regex"));
    for capture in markdown_link_re.captures_iter(text) {
        let label = capture
            .get(1)
            .map(|value| value.as_str().trim().to_string())
            .filter(|value| !value.is_empty());
        let Some(uri) = capture
            .get(2)
            .map(|value| value.as_str().trim().to_string())
        else {
            continue;
        };
        if seen.insert(uri.clone()) {
            artifacts.push(TaskArtifact {
                kind: infer_artifact_kind(&uri).to_string(),
                uri,
                label,
            });
        }
    }

    // Capture explicit absolute file paths enclosed in backticks to avoid pulling
    // random prose tokens into the artifact contract.
    let abs_path_re =
        ABS_PATH_RE.get_or_init(|| Regex::new(r"`(/[^`\s]+)`").expect("absolute path regex"));
    for capture in abs_path_re.captures_iter(text) {
        let Some(uri) = capture
            .get(1)
            .map(|value| value.as_str().trim().to_string())
        else {
            continue;
        };
        if seen.insert(uri.clone()) {
            artifacts.push(TaskArtifact {
                kind: "file".to_string(),
                uri,
                label: None,
            });
        }
    }

    artifacts
}

fn infer_artifact_kind(uri: &str) -> &'static str {
    if uri.starts_with("http://") || uri.starts_with("https://") {
        "url"
    } else if uri.starts_with('/') {
        "file"
    } else {
        "reference"
    }
}

#[cfg(test)]
mod tests {
    use super::{SubagentExecutor, SubagentRequest, SubagentResult, TaskTool, TaskToolInput};
    use crate::Result;
    use crate::{Tool, ToolExecutionContext};
    use agent_core_types::ToolCallId;
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct FakeSubagentExecutor {
        requests: Mutex<Vec<SubagentRequest>>,
    }

    #[async_trait]
    impl SubagentExecutor for FakeSubagentExecutor {
        async fn run(&self, request: SubagentRequest) -> Result<SubagentResult> {
            self.requests.lock().unwrap().push(request);
            Ok(SubagentResult {
                run_id: "run-child-1".to_string(),
                session_id: "session-child-1".to_string(),
                agent_name: "explorer".to_string(),
                assistant_text: "subagent completed".to_string(),
                allowed_tools: vec!["read".to_string(), "glob".to_string()],
            })
        }
    }

    #[tokio::test]
    async fn task_tool_delegates_to_executor() {
        let executor = Arc::new(FakeSubagentExecutor::default());
        let tool = TaskTool::new(executor.clone());
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(TaskToolInput {
                    prompt: Some("inspect repository".to_string()),
                    agent: Some("explorer".to_string()),
                    steer: Some("focus on test files".to_string()),
                    allowed_tools: Some(vec!["read".to_string(), "glob".to_string()]),
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        assert!(result.text_content().contains("subagent completed"));
        assert!(result.text_content().contains("run-child-1"));
        let requests = executor.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].prompt, "inspect repository");
        assert_eq!(requests[0].agent.as_deref(), Some("explorer"));
        assert_eq!(requests[0].steer.as_deref(), Some("focus on test files"));
        assert_eq!(
            requests[0].allowed_tools,
            Some(vec!["read".to_string(), "glob".to_string()])
        );
    }

    struct StructuredResponseExecutor {
        response_text: String,
    }

    #[async_trait]
    impl SubagentExecutor for StructuredResponseExecutor {
        async fn run(&self, _request: SubagentRequest) -> Result<SubagentResult> {
            Ok(SubagentResult {
                run_id: "run-child-2".to_string(),
                session_id: "session-child-2".to_string(),
                agent_name: "worker".to_string(),
                assistant_text: self.response_text.clone(),
                allowed_tools: vec!["read".to_string()],
            })
        }
    }

    #[tokio::test]
    async fn task_tool_extracts_structured_status_and_artifacts() {
        let executor = Arc::new(StructuredResponseExecutor {
            response_text: serde_json::json!({
                "status": "done",
                "summary": "patched files",
                "text": "Completed with artifact [spec](https://example.com/spec) and `/tmp/output.log`",
                "artifacts": [{"uri": "https://example.com/spec", "kind": "url", "label": "spec"}]
            })
            .to_string(),
        });
        let tool = TaskTool::new(executor);
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(TaskToolInput {
                    prompt: Some("run child".to_string()),
                    agent: None,
                    steer: None,
                    allowed_tools: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        let metadata = result.metadata.unwrap();
        assert_eq!(metadata["status"], "completed");
        assert_eq!(metadata["summary"], "patched files");
        assert_eq!(metadata["artifacts"][0]["uri"], "https://example.com/spec");
        assert_eq!(metadata["used_structured_payload"], true);
    }

    #[tokio::test]
    async fn task_tool_extracts_artifacts_from_plain_text() {
        let executor = Arc::new(StructuredResponseExecutor {
            response_text: "See [docs](https://example.com/docs) and `/Users/test/output.txt`"
                .to_string(),
        });
        let tool = TaskTool::new(executor);
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(TaskToolInput {
                    prompt: Some("run child".to_string()),
                    agent: None,
                    steer: None,
                    allowed_tools: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        let metadata = result.metadata.unwrap();
        assert_eq!(metadata["used_structured_payload"], false);
        assert_eq!(metadata["artifacts"].as_array().unwrap().len(), 2);
    }
}
