use crate::ToolExecutionContext;
use crate::annotations::mcp_tool_annotations;
use crate::registry::Tool;
use agent_core_types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};
use anyhow::{Result, bail};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct TaskToolInput {
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub task: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub agent_name: Option<String>,
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
        .or_else(|| input.task.clone())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("task requires prompt (or legacy task)"))
}

fn resolve_agent(input: &TaskToolInput) -> Option<String> {
    input
        .agent
        .clone()
        .or_else(|| input.agent_name.clone())
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
            bail!("task prompt must not be empty");
        }

        let output = self.executor.run(request).await?;
        let summary = format!(
            "[task agent={} run_id={} session_id={}]\nallowed_tools> {}\n\n{}",
            output.agent_name,
            output.run_id,
            output.session_id,
            if output.allowed_tools.is_empty() {
                "none".to_string()
            } else {
                output.allowed_tools.join(", ")
            },
            if output.assistant_text.trim().is_empty() {
                "[Subagent completed without textual output]".to_string()
            } else {
                output.assistant_text.clone()
            }
        );
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "task".to_string(),
            parts: vec![MessagePart::text(summary)],
            metadata: Some(serde_json::json!({
                "run_id": output.run_id,
                "session_id": output.session_id,
                "agent_name": output.agent_name,
                "allowed_tools": output.allowed_tools,
            })),
            is_error: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{SubagentExecutor, SubagentRequest, SubagentResult, TaskTool, TaskToolInput};
    use crate::{Tool, ToolExecutionContext};
    use agent_core_types::ToolCallId;
    use anyhow::Result;
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
                    task: None,
                    agent: Some("explorer".to_string()),
                    agent_name: None,
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

    #[tokio::test]
    async fn task_tool_accepts_legacy_task_and_agent_name_fields() {
        let executor = Arc::new(FakeSubagentExecutor::default());
        let tool = TaskTool::new(executor.clone());
        tool.execute(
            ToolCallId::new(),
            serde_json::to_value(TaskToolInput {
                prompt: None,
                task: Some("legacy task".to_string()),
                agent: None,
                agent_name: Some("legacy-agent".to_string()),
                steer: None,
                allowed_tools: None,
            })
            .unwrap(),
            &ToolExecutionContext::default(),
        )
        .await
        .unwrap();

        let requests = executor.requests.lock().unwrap();
        assert_eq!(requests[0].prompt, "legacy task");
        assert_eq!(requests[0].agent.as_deref(), Some("legacy-agent"));
    }
}
