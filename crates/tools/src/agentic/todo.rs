use crate::annotations::mcp_tool_annotations;
use crate::registry::Tool;
use crate::{Result, ToolExecutionContext};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};

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
        self.items.lock().expect("todo list lock").clone()
    }

    pub async fn replace(&self, items: Vec<TodoItem>) {
        *self.items.lock().expect("todo list lock") = items;
    }

    pub async fn merge(&self, items: Vec<TodoItem>) -> Vec<TodoItem> {
        let mut guard = self.items.lock().expect("todo list lock");
        for item in items {
            match guard.iter_mut().find(|existing| existing.id == item.id) {
                Some(existing) => *existing = item,
                None => guard.push(item),
            }
        }
        guard.clone()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct TodoReadInput {
    #[serde(default)]
    pub include_completed: bool,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct TodoReadToolOutput {
    count: usize,
    include_completed: bool,
    revision: String,
    items: Vec<TodoItem>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TodoWriteCommand {
    Replace,
    Merge,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct TodoWriteInput {
    pub items: Vec<TodoItem>,
    #[serde(default)]
    pub command: Option<TodoWriteCommand>,
    #[serde(default)]
    pub expected_revision: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TodoWriteToolOutput {
    Success {
        command: TodoWriteCommand,
        count: usize,
        revision_before: String,
        revision_after: String,
        items: Vec<TodoItem>,
    },
    Error {
        command: TodoWriteCommand,
        expected_revision: String,
        revision_before: String,
    },
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
            output_schema: Some(
                serde_json::to_value(schema_for!(TodoReadToolOutput))
                    .expect("todo_read output schema"),
            ),
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
        let external_call_id = types::CallId::from(&call_id);
        let input: TodoReadInput = serde_json::from_value(arguments)?;
        let snapshot = self.state.snapshot().await;
        let revision = revision_for(&snapshot);
        let items = if input.include_completed {
            snapshot
        } else {
            snapshot
                .into_iter()
                .filter(|item| item.status != TodoStatus::Completed)
                .collect()
        };
        let text = format!(
            "[todo count={} revision={} include_completed={}]\n{}",
            items.len(),
            revision,
            input.include_completed,
            render_todos(&items)
        );
        let structured_output = TodoReadToolOutput {
            count: items.len(),
            include_completed: input.include_completed,
            revision: revision.clone(),
            items: items.clone(),
        };
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "todo_read".to_string(),
            parts: vec![MessagePart::text(text)],
            structured_content: Some(
                serde_json::to_value(structured_output).expect("todo_read structured output"),
            ),
            metadata: Some(serde_json::json!({
                "count": items.len(),
                "include_completed": input.include_completed,
                "revision": revision,
                "items": items,
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
            description: "Replace or merge the shared todo list. Supports expected_revision guards so callers can detect stale todo snapshots."
                .to_string(),
            input_schema: serde_json::to_value(schema_for!(TodoWriteInput))
                .expect("todo_write schema"),
            output_mode: ToolOutputMode::Text,
            output_schema: Some(
                serde_json::to_value(schema_for!(TodoWriteToolOutput))
                    .expect("todo_write output schema"),
            ),
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
        let external_call_id = types::CallId::from(&call_id);
        let input: TodoWriteInput = serde_json::from_value(arguments)?;
        let command = input.command.clone().unwrap_or(TodoWriteCommand::Replace);
        let before = self.state.snapshot().await;
        let revision_before = revision_for(&before);
        if let Some(expected_revision) = input.expected_revision.as_deref()
            && expected_revision != revision_before
        {
            let structured_output = TodoWriteToolOutput::Error {
                command: command.clone(),
                expected_revision: expected_revision.to_string(),
                revision_before: revision_before.clone(),
            };
            return Ok(ToolResult {
                id: call_id,
                call_id: external_call_id.clone(),
                tool_name: "todo_write".to_string(),
                parts: vec![MessagePart::text(format!(
                    "Todo revision mismatch. Expected {expected_revision}, found {revision_before}. Re-read todos before writing."
                ))],
                structured_content: Some(
                    serde_json::to_value(structured_output)
                        .expect("todo_write error structured output"),
                ),
                metadata: Some(serde_json::json!({
                    "command": command,
                    "expected_revision": expected_revision,
                    "revision_before": revision_before,
                })),
                is_error: true,
            });
        }

        let updated = match command {
            TodoWriteCommand::Replace => {
                self.state.replace(input.items.clone()).await;
                input.items
            }
            TodoWriteCommand::Merge => self.state.merge(input.items.clone()).await,
        };
        let revision_after = revision_for(&updated);
        let summary = format!(
            "[todo_write command={:?} count={} revision {} -> {}]\n{}",
            command,
            updated.len(),
            revision_before,
            revision_after,
            render_todos(&updated)
        );
        let structured_output = TodoWriteToolOutput::Success {
            command: command.clone(),
            count: updated.len(),
            revision_before: revision_before.clone(),
            revision_after: revision_after.clone(),
            items: updated.clone(),
        };
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "todo_write".to_string(),
            parts: vec![MessagePart::text(summary)],
            structured_content: Some(
                serde_json::to_value(structured_output).expect("todo_write structured output"),
            ),
            metadata: Some(serde_json::json!({
                "command": command,
                "count": updated.len(),
                "revision_before": revision_before,
                "revision_after": revision_after,
                "items": updated,
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

fn revision_for(items: &[TodoItem]) -> String {
    crate::stable_text_hash(&serde_json::to_string(items).expect("todo revision json"))
}

#[cfg(test)]
mod tests {
    use super::{
        TodoItem, TodoListState, TodoReadInput, TodoReadTool, TodoStatus, TodoWriteCommand,
        TodoWriteInput, TodoWriteTool,
    };
    use crate::{Tool, ToolExecutionContext};
    use types::ToolCallId;

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
        assert!(text.contains("revision="));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["count"], 1);
        assert_eq!(structured["items"][0]["id"], "t2");
    }

    #[tokio::test]
    async fn todo_write_can_merge_by_id() {
        let state = TodoListState::new(sample_items());
        let writer = TodoWriteTool::new(state.clone());
        let result = writer
            .execute(
                ToolCallId::new(),
                serde_json::to_value(TodoWriteInput {
                    items: vec![TodoItem {
                        id: "t2".to_string(),
                        content: "Implement runtime queue".to_string(),
                        status: TodoStatus::Completed,
                    }],
                    command: Some(TodoWriteCommand::Merge),
                    expected_revision: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["kind"], "success");
        assert_eq!(structured["command"], "merge");
        assert_eq!(structured["items"].as_array().unwrap().len(), 2);
        let snapshot = state.snapshot().await;
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[1].status, TodoStatus::Completed);
    }
}
