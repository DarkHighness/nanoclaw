use crate::ToolExecutionContext;
use crate::annotations::mcp_tool_annotations;
use crate::registry::Tool;
use agent_core_types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};
use anyhow::Result;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: TodoStatus,
}

#[derive(Clone, Debug, Default)]
pub struct TodoListState {
    items: Arc<Mutex<Vec<TodoItem>>>,
}

impl TodoListState {
    #[must_use]
    pub fn new(initial_items: Vec<TodoItem>) -> Self {
        Self {
            items: Arc::new(Mutex::new(initial_items)),
        }
    }

    pub async fn snapshot(&self) -> Vec<TodoItem> {
        self.items.lock().await.clone()
    }

    pub async fn replace(&self, items: Vec<TodoItem>) {
        *self.items.lock().await = items;
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct TodoReadInput {
    #[serde(default)]
    pub include_completed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct TodoWriteInput {
    pub items: Vec<TodoItem>,
}

#[derive(Clone, Debug)]
pub struct TodoReadTool {
    state: TodoListState,
}

impl TodoReadTool {
    #[must_use]
    pub fn new(state: TodoListState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Tool for TodoReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "todo_read".to_string(),
            description: "Read the shared todo list for the current agent session.".to_string(),
            input_schema: serde_json::to_value(schema_for!(TodoReadInput))
                .expect("todo_read schema"),
            output_mode: ToolOutputMode::Text,
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("Read Todos", true, false, true, false),
        }
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        _ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = call_id.0.clone();
        let input: TodoReadInput = serde_json::from_value(arguments)?;
        let mut items = self.state.snapshot().await;
        if !input.include_completed {
            items.retain(|item| item.status != TodoStatus::Completed);
        }
        let text = render_todos(&items);
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "todo_read".to_string(),
            parts: vec![MessagePart::text(text)],
            metadata: Some(serde_json::json!({
                "count": items.len(),
                "include_completed": input.include_completed,
            })),
            is_error: false,
        })
    }
}

#[derive(Clone, Debug)]
pub struct TodoWriteTool {
    state: TodoListState,
}

impl TodoWriteTool {
    #[must_use]
    pub fn new(state: TodoListState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "todo_write".to_string(),
            description: "Replace the shared todo list with an updated set of todo items."
                .to_string(),
            input_schema: serde_json::to_value(schema_for!(TodoWriteInput))
                .expect("todo_write schema"),
            output_mode: ToolOutputMode::Text,
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("Write Todos", false, true, true, false),
        }
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        _ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = call_id.0.clone();
        let input: TodoWriteInput = serde_json::from_value(arguments)?;
        self.state.replace(input.items.clone()).await;

        let summary = format!(
            "Updated todo list with {} item(s).\n{}",
            input.items.len(),
            render_todos(&input.items)
        );
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "todo_write".to_string(),
            parts: vec![MessagePart::text(summary)],
            metadata: Some(serde_json::json!({
                "count": input.items.len(),
            })),
            is_error: false,
        })
    }
}

fn render_todos(items: &[TodoItem]) -> String {
    if items.is_empty() {
        return "No todos.".to_string();
    }
    items
        .iter()
        .map(|item| {
            format!(
                "- [{}] {} ({})",
                status_marker(&item.status),
                item.content,
                item.id
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn status_marker(status: &TodoStatus) -> &'static str {
    match status {
        TodoStatus::Pending => " ",
        TodoStatus::InProgress => "~",
        TodoStatus::Completed => "x",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TodoItem, TodoListState, TodoReadInput, TodoReadTool, TodoStatus, TodoWriteInput,
        TodoWriteTool,
    };
    use crate::{Tool, ToolExecutionContext};
    use agent_core_types::ToolCallId;

    fn sample_items() -> Vec<TodoItem> {
        vec![
            TodoItem {
                id: "t1".to_string(),
                content: "Inspect repository".to_string(),
                status: TodoStatus::Completed,
            },
            TodoItem {
                id: "t2".to_string(),
                content: "Implement runtime queue".to_string(),
                status: TodoStatus::InProgress,
            },
        ]
    }

    #[tokio::test]
    async fn todo_state_replace_and_snapshot_work() {
        let state = TodoListState::new(sample_items());
        assert_eq!(state.snapshot().await.len(), 2);
        state.replace(Vec::new()).await;
        assert!(state.snapshot().await.is_empty());
    }

    #[tokio::test]
    async fn todo_read_filters_completed_by_default() {
        let state = TodoListState::new(sample_items());
        let tool = TodoReadTool::new(state);
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(TodoReadInput::default()).unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let text = result.text_content();
        assert!(text.contains("runtime queue"));
        assert!(!text.contains("Inspect repository"));
    }

    #[tokio::test]
    async fn todo_write_replaces_items() {
        let state = TodoListState::new(Vec::new());
        let writer = TodoWriteTool::new(state.clone());
        let reader = TodoReadTool::new(state.clone());
        writer
            .execute(
                ToolCallId::new(),
                serde_json::to_value(TodoWriteInput {
                    items: sample_items(),
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert_eq!(state.snapshot().await.len(), 2);
        let result = reader
            .execute(
                ToolCallId::new(),
                serde_json::json!({"include_completed": true}),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert!(result.text_content().contains("Inspect repository"));
    }
}
