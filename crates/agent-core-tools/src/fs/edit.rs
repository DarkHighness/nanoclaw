use crate::ToolExecutionContext;
use crate::annotations::mcp_tool_annotations;
use crate::fs::{TextBuffer, stable_text_hash};
use crate::fs::{assert_path_inside_root, resolve_tool_path_against_workspace_root};
use crate::registry::Tool;
use agent_core_types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};
use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::fs;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EditCommand {
    StrReplace,
    ReplaceLines,
    Insert,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct EditToolInput {
    pub path: String,
    pub command: Option<EditCommand>,
    pub old_text: Option<String>,
    pub new_text: Option<String>,
    pub replace_all: Option<bool>,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
    pub insert_line: Option<usize>,
    pub text: Option<String>,
    pub expected_snapshot: Option<String>,
    pub expected_selection_hash: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct EditTool;

impl EditTool {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

fn resolve_command(input: &EditToolInput) -> Result<EditCommand> {
    if let Some(command) = &input.command {
        return Ok(command.clone());
    }
    if input.old_text.is_some() || input.new_text.is_some() {
        return Ok(EditCommand::StrReplace);
    }
    bail!("edit requires command, or legacy old_text/new_text fields");
}

fn run_string_replace(
    path: &str,
    content: String,
    input: &EditToolInput,
) -> Result<(String, String, Value)> {
    let old_text = input
        .old_text
        .as_deref()
        .ok_or_else(|| anyhow!("str_replace requires old_text"))?;
    let new_text = input
        .new_text
        .as_deref()
        .ok_or_else(|| anyhow!("str_replace requires new_text"))?;

    let occurrences = content.matches(old_text).count();
    if occurrences == 0 {
        return Ok((
            content,
            format!("No exact match found in {path}"),
            serde_json::json!({
                "command": "str_replace",
                "occurrences": 0,
                "replace_all": input.replace_all.unwrap_or(false),
                "is_error": true,
            }),
        ));
    }
    if occurrences > 1 && !input.replace_all.unwrap_or(false) {
        return Ok((
            content,
            format!(
                "Found {occurrences} matches in {path}. Re-run with replace_all=true or provide a more specific old_text"
            ),
            serde_json::json!({
                "command": "str_replace",
                "occurrences": occurrences,
                "replace_all": false,
                "is_error": true,
            }),
        ));
    }

    let updated = if input.replace_all.unwrap_or(false) {
        content.replace(old_text, new_text)
    } else {
        content.replacen(old_text, new_text, 1)
    };

    Ok((
        updated,
        format!(
            "Edited {path} using str_replace ({} replacement{})",
            if input.replace_all.unwrap_or(false) {
                occurrences
            } else {
                1
            },
            if input.replace_all.unwrap_or(false) && occurrences != 1 {
                "s"
            } else {
                ""
            }
        ),
        serde_json::json!({
            "command": "str_replace",
            "occurrences": occurrences,
            "replace_all": input.replace_all.unwrap_or(false),
        }),
    ))
}

fn run_line_replace(
    path: &str,
    content: String,
    input: &EditToolInput,
) -> Result<(String, String, Value)> {
    let start_line = input
        .start_line
        .ok_or_else(|| anyhow!("replace_lines requires start_line"))?;
    let end_line = input
        .end_line
        .ok_or_else(|| anyhow!("replace_lines requires end_line"))?;
    let replacement_text = input
        .text
        .as_deref()
        .or(input.new_text.as_deref())
        .ok_or_else(|| anyhow!("replace_lines requires text"))?;

    let mut buffer = TextBuffer::parse(&content);
    let current_slice = buffer.line_slice_text(start_line, end_line)?;
    let current_slice_hash = stable_text_hash(&current_slice);
    if let Some(expected_selection_hash) = &input.expected_selection_hash
        && expected_selection_hash != &current_slice_hash
    {
        return Ok((
            content,
            format!(
                "Slice mismatch for {path} lines {start_line}-{end_line}. Expected {}, found {}. Re-read that range before editing.",
                expected_selection_hash, current_slice_hash
            ),
            serde_json::json!({
                "command": "replace_lines",
                "start_line": start_line,
                "end_line": end_line,
                "current_slice_hash": current_slice_hash,
                "is_error": true,
            }),
        ));
    }

    buffer.replace_lines(start_line, end_line, replacement_text)?;
    Ok((
        buffer.to_text(),
        format!("Edited {path} using replace_lines ({start_line}-{end_line})"),
        serde_json::json!({
            "command": "replace_lines",
            "start_line": start_line,
            "end_line": end_line,
            "previous_slice_hash": current_slice_hash,
        }),
    ))
}

fn run_insert(
    path: &str,
    content: String,
    input: &EditToolInput,
) -> Result<(String, String, Value)> {
    let insert_line = input
        .insert_line
        .ok_or_else(|| anyhow!("insert requires insert_line"))?;
    let insert_text = input
        .text
        .as_deref()
        .or(input.new_text.as_deref())
        .ok_or_else(|| anyhow!("insert requires text"))?;

    let mut buffer = TextBuffer::parse(&content);
    buffer.insert_after(insert_line, insert_text)?;
    Ok((
        buffer.to_text(),
        format!("Edited {path} using insert (after line {insert_line})"),
        serde_json::json!({
            "command": "insert",
            "insert_line": insert_line,
        }),
    ))
}

#[async_trait]
impl Tool for EditTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit".to_string(),
            description: "Modify a UTF-8 file using str_replace, replace_lines, or insert. Use expected_snapshot or expected_selection_hash to guard against stale reads.".to_string(),
            input_schema: serde_json::to_value(schema_for!(EditToolInput)).expect("edit schema"),
            output_mode: ToolOutputMode::Text,
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("Edit File", false, true, true, false),
        }
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = call_id.0.clone();
        let input: EditToolInput = serde_json::from_value(arguments)?;
        let resolved = resolve_tool_path_against_workspace_root(
            &input.path,
            ctx.effective_root(),
            ctx.container_workdir.as_deref(),
        )?;
        if ctx.workspace_only {
            assert_path_inside_root(&resolved, ctx.effective_root())?;
        }
        let content = fs::read_to_string(&resolved).await?;
        let snapshot_before = stable_text_hash(&content);
        if let Some(expected_snapshot) = &input.expected_snapshot
            && expected_snapshot != &snapshot_before
        {
            return Ok(ToolResult::error(
                call_id,
                "edit",
                format!(
                    "Snapshot mismatch for {}. Expected {}, found {}. Re-read the file before editing.",
                    input.path, expected_snapshot, snapshot_before
                ),
            ));
        }

        let command = resolve_command(&input)?;
        let (updated, summary, metadata, is_error) = match command {
            EditCommand::StrReplace => {
                let (updated, summary, metadata) =
                    run_string_replace(&input.path, content, &input)?;
                let is_error = metadata
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                (updated, summary, metadata, is_error)
            }
            EditCommand::ReplaceLines => {
                let (updated, summary, metadata) = run_line_replace(&input.path, content, &input)?;
                let is_error = metadata
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                (updated, summary, metadata, is_error)
            }
            EditCommand::Insert => {
                let (updated, summary, metadata) = run_insert(&input.path, content, &input)?;
                (updated, summary, metadata, false)
            }
        };

        if is_error {
            return Ok(ToolResult::error(call_id, "edit", summary));
        }

        let snapshot_after = stable_text_hash(&updated);
        fs::write(&resolved, updated.as_bytes()).await?;

        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "edit".to_string(),
            parts: vec![MessagePart::text(format!(
                "{summary}\n[snapshot {} -> {}]",
                snapshot_before, snapshot_after
            ))],
            metadata: Some(serde_json::json!({
                "path": resolved,
                "snapshot_before": snapshot_before,
                "snapshot_after": snapshot_after,
                "edit": metadata,
            })),
            is_error: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{EditCommand, EditTool, EditToolInput};
    use crate::{Tool, ToolExecutionContext, stable_text_hash};
    use agent_core_types::ToolCallId;

    #[tokio::test]
    async fn edit_tool_replaces_exact_match() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.txt");
        tokio::fs::write(&path, "hello world\n").await.unwrap();

        let tool = EditTool::new();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(EditToolInput {
                    path: "sample.txt".to_string(),
                    command: None,
                    old_text: Some("world".to_string()),
                    new_text: Some("agent".to_string()),
                    replace_all: None,
                    start_line: None,
                    end_line: None,
                    insert_line: None,
                    text: None,
                    expected_snapshot: None,
                    expected_selection_hash: None,
                })
                .unwrap(),
                &ToolExecutionContext {
                    workspace_root: dir.path().to_path_buf(),
                    sandbox_root: None,
                    workspace_only: true,
                    container_workdir: None,
                    model_context_window_tokens: None,
                },
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(
            tokio::fs::read_to_string(path).await.unwrap(),
            "hello agent\n"
        );
    }

    #[tokio::test]
    async fn edit_tool_replaces_line_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.txt");
        tokio::fs::write(&path, "alpha\nbeta\ngamma\n")
            .await
            .unwrap();

        let tool = EditTool::new();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(EditToolInput {
                    path: "sample.txt".to_string(),
                    command: Some(EditCommand::ReplaceLines),
                    old_text: None,
                    new_text: None,
                    replace_all: None,
                    start_line: Some(2),
                    end_line: Some(3),
                    insert_line: None,
                    text: Some("bravo\ncharlie".to_string()),
                    expected_snapshot: None,
                    expected_selection_hash: Some(stable_text_hash("beta\ngamma")),
                })
                .unwrap(),
                &ToolExecutionContext {
                    workspace_root: dir.path().to_path_buf(),
                    sandbox_root: None,
                    workspace_only: true,
                    container_workdir: None,
                    model_context_window_tokens: None,
                },
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(
            tokio::fs::read_to_string(path).await.unwrap(),
            "alpha\nbravo\ncharlie\n"
        );
    }

    #[tokio::test]
    async fn edit_tool_inserts_after_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.txt");
        tokio::fs::write(&path, "alpha\nbeta\n").await.unwrap();

        let tool = EditTool::new();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(EditToolInput {
                    path: "sample.txt".to_string(),
                    command: Some(EditCommand::Insert),
                    old_text: None,
                    new_text: None,
                    replace_all: None,
                    start_line: None,
                    end_line: None,
                    insert_line: Some(1),
                    text: Some("inserted".to_string()),
                    expected_snapshot: None,
                    expected_selection_hash: None,
                })
                .unwrap(),
                &ToolExecutionContext {
                    workspace_root: dir.path().to_path_buf(),
                    sandbox_root: None,
                    workspace_only: true,
                    container_workdir: None,
                    model_context_window_tokens: None,
                },
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(
            tokio::fs::read_to_string(path).await.unwrap(),
            "alpha\ninserted\nbeta\n"
        );
    }
}
