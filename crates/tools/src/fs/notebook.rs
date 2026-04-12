use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::file_activity::FileActivityObserver;
use crate::fs::{
    commit_text_file, compute_diff_preview, resolve_tool_path_against_workspace_root,
    stable_text_hash,
};
use crate::registry::Tool;
use crate::{Result, ToolError, ToolExecutionContext};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::fs;
use types::{MessagePart, ToolCallId, ToolOutputMode, ToolResult, ToolSpec};

const NOTEBOOK_EDIT_TOOL_NAME: &str = "notebook_edit";
const NOTEBOOK_READ_TOOL_NAME: &str = "notebook_read";
const DEFAULT_NOTEBOOK_CELL_COUNT: usize = 8;
const MAX_NOTEBOOK_CELL_COUNT: usize = 64;
const MAX_SOURCE_PREVIEW_LINES: usize = 4;
const MAX_OUTPUT_PREVIEW_LINES: usize = 3;
const MAX_OUTPUT_PREVIEW_ITEMS: usize = 3;
const MAX_PREVIEW_COLUMNS: usize = 96;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct NotebookReadToolInput {
    pub path: String,
    #[serde(default)]
    pub start_cell: Option<usize>,
    pub end_cell: Option<usize>,
    #[serde(default)]
    pub cell_count: Option<usize>,
    #[serde(default)]
    pub include_outputs: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct NotebookEditToolInput {
    pub path: String,
    pub operations: Vec<NotebookEditOperation>,
    #[serde(default)]
    pub expected_snapshot: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotebookEditableCellType {
    Code,
    Markdown,
    Raw,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum NotebookEditOperation {
    ReplaceCell {
        cell_index: usize,
        cell_type: NotebookEditableCellType,
        source: String,
    },
    InsertCell {
        cell_index: usize,
        cell_type: NotebookEditableCellType,
        source: String,
    },
    DeleteCell {
        cell_index: usize,
    },
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum NotebookCellType {
    Code,
    Markdown,
    Raw,
    Unknown,
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum NotebookOutputType {
    Stream,
    ExecuteResult,
    DisplayData,
    Error,
    Unknown,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct NotebookOutputPreview {
    kind: NotebookOutputType,
    summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    text_preview: Vec<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct NotebookCellPreview {
    index: usize,
    cell_type: NotebookCellType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    execution_count: Option<i64>,
    source_line_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    source_preview: Vec<String>,
    output_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    outputs: Vec<NotebookOutputPreview>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct NotebookReadToolOutput {
    requested_path: String,
    resolved_path: String,
    nbformat: u64,
    nbformat_minor: u64,
    total_cells: usize,
    start_cell: usize,
    end_cell: usize,
    output_cells: usize,
    remaining_cells: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_start_cell: Option<usize>,
    include_outputs: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    language_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kernelspec_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kernelspec_display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    cells: Vec<NotebookCellPreview>,
    empty: bool,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum NotebookAppliedEdit {
    ReplaceCell {
        cell_index: usize,
        cell_type: NotebookEditableCellType,
    },
    InsertCell {
        cell_index: usize,
        cell_type: NotebookEditableCellType,
    },
    DeleteCell {
        cell_index: usize,
    },
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum NotebookEditToolOutput {
    Success {
        requested_path: String,
        resolved_path: String,
        summary: String,
        snapshot_before: String,
        snapshot_after: String,
        cell_count_before: usize,
        cell_count_after: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        language_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kernelspec_name: Option<String>,
        operations: Vec<NotebookAppliedEdit>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        changed_cells: Vec<NotebookCellPreview>,
        file_diffs: Vec<Value>,
    },
    Error {
        requested_path: String,
        resolved_path: String,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        snapshot_before: Option<String>,
        operation_count: usize,
    },
}

#[derive(Clone, Default)]
pub struct NotebookReadTool {
    activity_observer: Option<Arc<dyn FileActivityObserver>>,
}

#[derive(Clone, Default)]
pub struct NotebookEditTool {
    activity_observer: Option<Arc<dyn FileActivityObserver>>,
}

impl NotebookReadTool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            activity_observer: None,
        }
    }

    #[must_use]
    pub fn with_file_activity_observer(activity_observer: Arc<dyn FileActivityObserver>) -> Self {
        Self {
            activity_observer: Some(activity_observer),
        }
    }
}

impl NotebookEditTool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            activity_observer: None,
        }
    }

    #[must_use]
    pub fn with_file_activity_observer(activity_observer: Arc<dyn FileActivityObserver>) -> Self {
        Self {
            activity_observer: Some(activity_observer),
        }
    }
}

#[async_trait]
impl Tool for NotebookEditTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            NOTEBOOK_EDIT_TOOL_NAME,
            "Edit an existing Jupyter notebook (`.ipynb`) through typed cell operations. Use expected_snapshot to guard against stale reads. Replace and insert operations create clean cells, and code-cell outputs are reset instead of carrying stale execution state forward.",
            serde_json::to_value(schema_for!(NotebookEditToolInput))
                .expect("notebook_edit schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, true, true, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(NotebookEditToolOutput))
                .expect("notebook_edit output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: NotebookEditToolInput = serde_json::from_value(arguments)?;
        if input.operations.is_empty() {
            return Err(ToolError::invalid(
                "notebook_edit requires at least one operation",
            ));
        }

        let resolved = resolve_tool_path_against_workspace_root(
            &input.path,
            ctx.effective_root(),
            ctx.container_workdir.as_deref(),
        )?;
        ctx.assert_path_write_allowed(&resolved)?;
        if resolved.extension().and_then(|value| value.to_str()) != Some("ipynb") {
            return Err(ToolError::invalid(
                "notebook_edit requires a .ipynb notebook path",
            ));
        }

        let before = fs::read_to_string(&resolved).await?;
        let snapshot_before = stable_text_hash(&before);
        if let Some(expected_snapshot) = input.expected_snapshot.as_deref()
            && expected_snapshot != snapshot_before
        {
            let structured_output = NotebookEditToolOutput::Error {
                requested_path: input.path.clone(),
                resolved_path: resolved.display().to_string(),
                summary: format!(
                    "Snapshot mismatch for {}. Expected {expected_snapshot}, found {snapshot_before}. Re-read the notebook before editing.",
                    input.path
                ),
                snapshot_before: Some(snapshot_before.clone()),
                operation_count: input.operations.len(),
            };
            return Ok(ToolResult {
                id: call_id,
                call_id: external_call_id,
                tool_name: NOTEBOOK_EDIT_TOOL_NAME.into(),
                parts: vec![MessagePart::text(match &structured_output {
                    NotebookEditToolOutput::Error { summary, .. } => summary.clone(),
                    NotebookEditToolOutput::Success { .. } => unreachable!(),
                })],
                attachments: Vec::new(),
                structured_content: Some(
                    serde_json::to_value(structured_output).expect("notebook_edit error output"),
                ),
                continuation: None,
                metadata: Some(json!({
                    "path": resolved,
                    "snapshot_before": snapshot_before,
                    "operation_count": input.operations.len(),
                })),
                is_error: true,
            });
        }

        let mut notebook: Value = serde_json::from_str(&before)
            .map_err(|_| ToolError::invalid("notebook_edit: file is not valid notebook JSON"))?;
        let cell_count_before = notebook
            .get("cells")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let language_name = notebook_language_name(&notebook);
        let kernelspec_name = notebook_kernelspec_name(&notebook);

        let mut applied = Vec::new();
        let mut changed_indices = Vec::new();
        for operation in input.operations {
            apply_notebook_edit_operation(
                &mut notebook,
                operation,
                &mut applied,
                &mut changed_indices,
            )?;
        }

        let after = serde_json::to_string_pretty(&notebook)
            .map_err(|_| ToolError::invalid("notebook_edit: failed to serialize notebook"))?;
        let snapshot_after = stable_text_hash(&after);
        commit_text_file(&resolved, Some(&after)).await?;
        if let Some(observer) = &self.activity_observer {
            observer.did_change(resolved.clone());
            observer.did_save(resolved.clone());
        }

        let file_diffs = compute_diff_preview(&input.path, Some(&before), Some(&after))
            .into_iter()
            .collect::<Vec<_>>();
        let changed_cells = collect_changed_cell_previews(&notebook, &changed_indices);
        let cell_count_after = notebook
            .get("cells")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let summary = format!(
            "Updated {} with {} notebook operation(s)",
            input.path,
            applied.len()
        );
        let structured_output = NotebookEditToolOutput::Success {
            requested_path: input.path.clone(),
            resolved_path: resolved.display().to_string(),
            summary: summary.clone(),
            snapshot_before: snapshot_before.clone(),
            snapshot_after: snapshot_after.clone(),
            cell_count_before,
            cell_count_after,
            language_name,
            kernelspec_name,
            operations: applied.clone(),
            changed_cells: changed_cells.clone(),
            file_diffs: file_diffs.clone(),
        };

        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: NOTEBOOK_EDIT_TOOL_NAME.into(),
            parts: vec![MessagePart::text(render_notebook_edit_text(
                &summary,
                &snapshot_before,
                &snapshot_after,
                &applied,
                &changed_cells,
            ))],
            attachments: Vec::new(),
            structured_content: Some(
                serde_json::to_value(structured_output).expect("notebook_edit success output"),
            ),
            continuation: None,
            metadata: Some(json!({
                "path": resolved,
                "snapshot_before": snapshot_before,
                "snapshot_after": snapshot_after,
                "cell_count_before": cell_count_before,
                "cell_count_after": cell_count_after,
                "operations": applied,
                "file_diffs": file_diffs,
            })),
            is_error: false,
        })
    }
}

#[async_trait]
impl Tool for NotebookReadTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            NOTEBOOK_READ_TOOL_NAME,
            "Read a Jupyter notebook (`.ipynb`) as typed notebook cells instead of raw JSON. Supports cell windows via `start_cell`/`end_cell` or `cell_count` and can include compact output previews for code cells.",
            serde_json::to_value(schema_for!(NotebookReadToolInput))
                .expect("notebook_read schema"),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, true, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(NotebookReadToolOutput))
                .expect("notebook_read output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: NotebookReadToolInput = serde_json::from_value(arguments)?;
        if input.end_cell.is_some() && input.cell_count.is_some() {
            return Err(ToolError::invalid(
                "notebook_read accepts either end_cell or cell_count, not both",
            ));
        }

        let resolved = resolve_tool_path_against_workspace_root(
            &input.path,
            ctx.effective_root(),
            ctx.container_workdir.as_deref(),
        )?;
        ctx.assert_path_read_allowed(&resolved)?;
        if resolved.extension().and_then(|value| value.to_str()) != Some("ipynb") {
            return Err(ToolError::invalid(
                "notebook_read requires a .ipynb notebook path",
            ));
        }

        let text = fs::read_to_string(&resolved).await?;
        if let Some(observer) = &self.activity_observer {
            observer.did_open(resolved.clone());
        }
        // Notebook inspection stays cell-oriented on purpose so follow-up
        // notebook tools do not depend on raw JSON offsets or brittle patches.
        let notebook: Value = serde_json::from_str(&text)
            .map_err(|_| ToolError::invalid("notebook_read: file is not valid notebook JSON"))?;
        let cells = notebook
            .get("cells")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ToolError::invalid("notebook_read: notebook is missing a cells array")
            })?;

        let total_cells = cells.len();
        let include_outputs = input.include_outputs.unwrap_or(true);
        let requested_count = input
            .cell_count
            .unwrap_or(DEFAULT_NOTEBOOK_CELL_COUNT)
            .clamp(1, MAX_NOTEBOOK_CELL_COUNT);

        if total_cells == 0 {
            let structured_output = NotebookReadToolOutput {
                requested_path: input.path.clone(),
                resolved_path: resolved.display().to_string(),
                nbformat: notebook
                    .get("nbformat")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                nbformat_minor: notebook
                    .get("nbformat_minor")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                total_cells: 0,
                start_cell: 0,
                end_cell: 0,
                output_cells: 0,
                remaining_cells: 0,
                next_start_cell: None,
                include_outputs,
                language_name: notebook_language_name(&notebook),
                kernelspec_name: notebook_kernelspec_name(&notebook),
                kernelspec_display_name: notebook_kernelspec_display_name(&notebook),
                cells: Vec::new(),
                empty: true,
            };
            return Ok(ToolResult {
                id: call_id,
                call_id: external_call_id,
                tool_name: NOTEBOOK_READ_TOOL_NAME.into(),
                parts: vec![MessagePart::text(format!(
                    "[notebook_read path={} cells=0/0]\n[Notebook has no cells]",
                    input.path
                ))],
                attachments: Vec::new(),
                structured_content: Some(
                    serde_json::to_value(structured_output).expect("notebook_read empty output"),
                ),
                continuation: None,
                metadata: Some(serde_json::json!({
                    "path": resolved,
                    "total_cells": 0,
                    "include_outputs": include_outputs,
                })),
                is_error: false,
            });
        }

        let start_cell = input.start_cell.unwrap_or(1).clamp(1, total_cells);
        let end_cell = input
            .end_cell
            .unwrap_or_else(|| start_cell.saturating_add(requested_count).saturating_sub(1))
            .clamp(start_cell, total_cells);
        let next_start_cell = (end_cell < total_cells).then_some(end_cell + 1);
        let output_cells = end_cell.saturating_sub(start_cell).saturating_add(1);
        let remaining_cells = total_cells.saturating_sub(end_cell);

        let cell_previews = cells[start_cell - 1..end_cell]
            .iter()
            .enumerate()
            .map(|(offset, cell)| notebook_cell_preview(cell, start_cell + offset, include_outputs))
            .collect::<Vec<_>>();

        let structured_output = NotebookReadToolOutput {
            requested_path: input.path.clone(),
            resolved_path: resolved.display().to_string(),
            nbformat: notebook
                .get("nbformat")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            nbformat_minor: notebook
                .get("nbformat_minor")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            total_cells,
            start_cell,
            end_cell,
            output_cells,
            remaining_cells,
            next_start_cell,
            include_outputs,
            language_name: notebook_language_name(&notebook),
            kernelspec_name: notebook_kernelspec_name(&notebook),
            kernelspec_display_name: notebook_kernelspec_display_name(&notebook),
            cells: cell_previews.clone(),
            empty: false,
        };
        let text_output = render_notebook_read_text(&structured_output);

        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: NOTEBOOK_READ_TOOL_NAME.into(),
            parts: vec![MessagePart::text(text_output)],
            attachments: Vec::new(),
            structured_content: Some(
                serde_json::to_value(structured_output).expect("notebook_read output"),
            ),
            continuation: None,
            metadata: Some(serde_json::json!({
                "path": resolved,
                "start_cell": start_cell,
                "end_cell": end_cell,
                "total_cells": total_cells,
                "include_outputs": include_outputs,
                "next_start_cell": next_start_cell,
            })),
            is_error: false,
        })
    }
}

fn notebook_cell_preview(cell: &Value, index: usize, include_outputs: bool) -> NotebookCellPreview {
    let cell_type = notebook_cell_type(cell.get("cell_type").and_then(Value::as_str));
    let source_lines = notebook_multiline_lines(cell.get("source"));
    let output_values = cell
        .get("outputs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let outputs = if include_outputs {
        output_values
            .iter()
            .take(MAX_OUTPUT_PREVIEW_ITEMS)
            .map(notebook_output_preview)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    NotebookCellPreview {
        index,
        cell_type,
        execution_count: cell.get("execution_count").and_then(Value::as_i64),
        source_line_count: source_lines.len(),
        source_preview: collapse_lines(&source_lines, MAX_SOURCE_PREVIEW_LINES),
        output_count: output_values.len(),
        outputs,
    }
}

fn notebook_cell_type(raw: Option<&str>) -> NotebookCellType {
    match raw.unwrap_or_default() {
        "code" => NotebookCellType::Code,
        "markdown" => NotebookCellType::Markdown,
        "raw" => NotebookCellType::Raw,
        _ => NotebookCellType::Unknown,
    }
}

fn notebook_output_preview(output: &Value) -> NotebookOutputPreview {
    let output_type_raw = output
        .get("output_type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match output_type_raw {
        "stream" => {
            let name = output
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("stream");
            let preview = collapse_lines(
                &notebook_multiline_lines(output.get("text")),
                MAX_OUTPUT_PREVIEW_LINES,
            );
            NotebookOutputPreview {
                kind: NotebookOutputType::Stream,
                summary: name.to_string(),
                text_preview: preview,
            }
        }
        "execute_result" => NotebookOutputPreview {
            kind: NotebookOutputType::ExecuteResult,
            summary: "text/plain result".to_string(),
            text_preview: collapse_lines(
                &notebook_text_plain_lines(output),
                MAX_OUTPUT_PREVIEW_LINES,
            ),
        },
        "display_data" => {
            let mime_types = output
                .get("data")
                .and_then(Value::as_object)
                .map(|data| data.keys().take(3).cloned().collect::<Vec<_>>().join(", "))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "display data".to_string());
            NotebookOutputPreview {
                kind: NotebookOutputType::DisplayData,
                summary: mime_types,
                text_preview: collapse_lines(
                    &notebook_text_plain_lines(output),
                    MAX_OUTPUT_PREVIEW_LINES,
                ),
            }
        }
        "error" => {
            let ename = output
                .get("ename")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("error");
            let evalue = output
                .get("evalue")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or_default();
            let summary = if evalue.is_empty() {
                ename.to_string()
            } else {
                format!("{ename}: {evalue}")
            };
            NotebookOutputPreview {
                kind: NotebookOutputType::Error,
                summary,
                text_preview: collapse_lines(
                    &notebook_multiline_lines(output.get("traceback")),
                    MAX_OUTPUT_PREVIEW_LINES,
                ),
            }
        }
        other => NotebookOutputPreview {
            kind: NotebookOutputType::Unknown,
            summary: if other.trim().is_empty() {
                "unknown".to_string()
            } else {
                other.to_string()
            },
            text_preview: Vec::new(),
        },
    }
}

fn notebook_multiline_lines(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(text)) => split_lines(text),
        Some(Value::Array(values)) => {
            let joined = values.iter().filter_map(Value::as_str).collect::<String>();
            split_lines(&joined)
        }
        _ => Vec::new(),
    }
}

fn notebook_text_plain_lines(output: &Value) -> Vec<String> {
    output
        .get("data")
        .and_then(Value::as_object)
        .and_then(|data| data.get("text/plain"))
        .map_or_else(Vec::new, |value| notebook_multiline_lines(Some(value)))
}

fn split_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim_end)
        .map(str::to_string)
        .collect()
}

fn collapse_lines(lines: &[String], max_lines: usize) -> Vec<String> {
    let clipped = lines
        .iter()
        .map(|line| clip_inline(line, MAX_PREVIEW_COLUMNS))
        .collect::<Vec<_>>();
    if clipped.len() <= max_lines.max(1) {
        return clipped;
    }

    let keep = max_lines.saturating_sub(1).max(1);
    let hidden = clipped.len().saturating_sub(keep);
    let mut preview = clipped.into_iter().take(keep).collect::<Vec<_>>();
    preview.push(format!("… +{hidden} lines"));
    preview
}

fn clip_inline(value: &str, max_columns: usize) -> String {
    if value.chars().count() > max_columns {
        format!(
            "{}...",
            value
                .chars()
                .take(max_columns.saturating_sub(3))
                .collect::<String>()
        )
    } else {
        value.to_string()
    }
}

fn render_notebook_read_text(output: &NotebookReadToolOutput) -> String {
    let mut lines = vec![format!(
        "[notebook_read path={} cells={}-{} / {}]",
        output.requested_path, output.start_cell, output.end_cell, output.total_cells
    )];
    if let Some(language_name) = output.language_name.as_deref() {
        lines.push(format!("language {language_name}"));
    }
    if let Some(kernelspec_name) = output.kernelspec_name.as_deref() {
        lines.push(format!("kernel {kernelspec_name}"));
    }

    for cell in &output.cells {
        lines.push(String::new());
        lines.push(render_notebook_cell_header(cell));
        if cell.source_preview.is_empty() {
            lines.push("  <empty>".to_string());
        } else {
            lines.extend(cell.source_preview.iter().map(|line| format!("  {line}")));
        }

        if output.include_outputs && !cell.outputs.is_empty() {
            for preview in &cell.outputs {
                lines.push(format!(
                    "  [output {}] {}",
                    notebook_output_label(preview),
                    preview.summary
                ));
                lines.extend(
                    preview
                        .text_preview
                        .iter()
                        .map(|line| format!("    {line}")),
                );
            }
        }
    }

    if let Some(next_start_cell) = output.next_start_cell {
        lines.push(String::new());
        lines.push(format!("[next_start_cell {next_start_cell}]"));
    }
    lines.join("\n")
}

fn render_notebook_cell_header(cell: &NotebookCellPreview) -> String {
    let cell_type = notebook_cell_type_label(cell.cell_type);
    match cell.execution_count {
        Some(execution_count) => format!(
            "Cell {} [{cell_type} exec={execution_count}] {} line(s), {} output(s)",
            cell.index, cell.source_line_count, cell.output_count
        ),
        None => format!(
            "Cell {} [{cell_type}] {} line(s), {} output(s)",
            cell.index, cell.source_line_count, cell.output_count
        ),
    }
}

fn notebook_output_label(output: &NotebookOutputPreview) -> &'static str {
    match output.kind {
        NotebookOutputType::Stream => "stream",
        NotebookOutputType::ExecuteResult => "execute_result",
        NotebookOutputType::DisplayData => "display_data",
        NotebookOutputType::Error => "error",
        NotebookOutputType::Unknown => "unknown",
    }
}

fn notebook_cell_type_label(cell_type: NotebookCellType) -> &'static str {
    match cell_type {
        NotebookCellType::Code => "code",
        NotebookCellType::Markdown => "markdown",
        NotebookCellType::Raw => "raw",
        NotebookCellType::Unknown => "unknown",
    }
}

fn notebook_language_name(notebook: &Value) -> Option<String> {
    notebook
        .get("metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("language_info"))
        .and_then(Value::as_object)
        .and_then(|language_info| language_info.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn notebook_kernelspec_name(notebook: &Value) -> Option<String> {
    notebook
        .get("metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("kernelspec"))
        .and_then(Value::as_object)
        .and_then(|kernelspec| kernelspec.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn notebook_kernelspec_display_name(notebook: &Value) -> Option<String> {
    notebook
        .get("metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("kernelspec"))
        .and_then(Value::as_object)
        .and_then(|kernelspec| kernelspec.get("display_name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn apply_notebook_edit_operation(
    notebook: &mut Value,
    operation: NotebookEditOperation,
    applied: &mut Vec<NotebookAppliedEdit>,
    changed_indices: &mut Vec<usize>,
) -> Result<()> {
    let cells = notebook
        .get_mut("cells")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| ToolError::invalid("notebook_edit: notebook is missing a cells array"))?;
    match operation {
        NotebookEditOperation::ReplaceCell {
            cell_index,
            cell_type,
            source,
        } => {
            if !(1..=cells.len()).contains(&cell_index) {
                return Err(ToolError::invalid(format!(
                    "notebook_edit replace_cell index {cell_index} is out of range for {} cells",
                    cells.len()
                )));
            }
            let metadata = cells[cell_index - 1]
                .get("metadata")
                .cloned()
                .unwrap_or_else(|| json!({}));
            cells[cell_index - 1] = build_notebook_cell(cell_type, &source, metadata);
            applied.push(NotebookAppliedEdit::ReplaceCell {
                cell_index,
                cell_type,
            });
            push_changed_index(changed_indices, cell_index);
        }
        NotebookEditOperation::InsertCell {
            cell_index,
            cell_type,
            source,
        } => {
            if !(1..=cells.len() + 1).contains(&cell_index) {
                return Err(ToolError::invalid(format!(
                    "notebook_edit insert_cell index {cell_index} is out of range for insertion into {} cells",
                    cells.len()
                )));
            }
            cells.insert(
                cell_index - 1,
                build_notebook_cell(cell_type, &source, json!({})),
            );
            applied.push(NotebookAppliedEdit::InsertCell {
                cell_index,
                cell_type,
            });
            push_changed_index(changed_indices, cell_index);
        }
        NotebookEditOperation::DeleteCell { cell_index } => {
            if !(1..=cells.len()).contains(&cell_index) {
                return Err(ToolError::invalid(format!(
                    "notebook_edit delete_cell index {cell_index} is out of range for {} cells",
                    cells.len()
                )));
            }
            cells.remove(cell_index - 1);
            applied.push(NotebookAppliedEdit::DeleteCell { cell_index });
        }
    }
    Ok(())
}

fn build_notebook_cell(
    cell_type: NotebookEditableCellType,
    source: &str,
    metadata: Value,
) -> Value {
    let source = notebook_source_value(source);
    match cell_type {
        NotebookEditableCellType::Code => json!({
            "cell_type": "code",
            "metadata": metadata,
            "source": source,
            "execution_count": Value::Null,
            "outputs": [],
        }),
        NotebookEditableCellType::Markdown => json!({
            "cell_type": "markdown",
            "metadata": metadata,
            "source": source,
        }),
        NotebookEditableCellType::Raw => json!({
            "cell_type": "raw",
            "metadata": metadata,
            "source": source,
        }),
    }
}

fn notebook_source_value(source: &str) -> Value {
    let lines = if source.is_empty() {
        Vec::new()
    } else {
        source
            .split_inclusive('\n')
            .map(|line| Value::String(line.to_string()))
            .collect::<Vec<_>>()
    };
    Value::Array(lines)
}

fn push_changed_index(changed_indices: &mut Vec<usize>, index: usize) {
    if !changed_indices.contains(&index) {
        changed_indices.push(index);
    }
}

fn collect_changed_cell_previews(
    notebook: &Value,
    changed_indices: &[usize],
) -> Vec<NotebookCellPreview> {
    let Some(cells) = notebook.get("cells").and_then(Value::as_array) else {
        return Vec::new();
    };
    changed_indices
        .iter()
        .copied()
        .filter_map(|index| cells.get(index.saturating_sub(1)).map(|cell| (index, cell)))
        .map(|(index, cell)| notebook_cell_preview(cell, index, true))
        .collect()
}

fn render_notebook_edit_text(
    summary: &str,
    snapshot_before: &str,
    snapshot_after: &str,
    operations: &[NotebookAppliedEdit],
    changed_cells: &[NotebookCellPreview],
) -> String {
    let mut lines = vec![
        summary.to_string(),
        format!("[snapshot {snapshot_before} -> {snapshot_after}]"),
    ];
    for operation in operations {
        lines.push(render_notebook_edit_operation(operation));
    }
    for cell in changed_cells {
        lines.push(String::new());
        lines.push(render_notebook_cell_header(cell));
        lines.extend(cell.source_preview.iter().map(|line| format!("  {line}")));
    }
    lines.join("\n")
}

fn render_notebook_edit_operation(operation: &NotebookAppliedEdit) -> String {
    match operation {
        NotebookAppliedEdit::ReplaceCell {
            cell_index,
            cell_type,
        } => format!(
            "replaced cell {cell_index} as {}",
            notebook_editable_cell_type_label(*cell_type)
        ),
        NotebookAppliedEdit::InsertCell {
            cell_index,
            cell_type,
        } => format!(
            "inserted {} cell at {cell_index}",
            notebook_editable_cell_type_label(*cell_type)
        ),
        NotebookAppliedEdit::DeleteCell { cell_index } => format!("deleted cell {cell_index}"),
    }
}

fn notebook_editable_cell_type_label(cell_type: NotebookEditableCellType) -> &'static str {
    match cell_type {
        NotebookEditableCellType::Code => "code",
        NotebookEditableCellType::Markdown => "markdown",
        NotebookEditableCellType::Raw => "raw",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        NotebookEditOperation, NotebookEditTool, NotebookEditToolInput, NotebookEditableCellType,
        NotebookReadTool, NotebookReadToolInput,
    };
    use crate::{Tool, ToolExecutionContext};
    use nanoclaw_test_support::run_current_thread_test;
    use serde_json::Value;
    use types::{MessagePart, ToolCallId};

    macro_rules! bounded_async_test {
        (async fn $name:ident() $body:block) => {
            #[test]
            fn $name() {
                run_current_thread_test(async $body);
            }
        };
    }

    fn sample_notebook() -> &'static str {
        r##"{
  "nbformat": 4,
  "nbformat_minor": 5,
  "metadata": {
    "kernelspec": {"name": "python3", "display_name": "Python 3"},
    "language_info": {"name": "python"}
  },
  "cells": [
    {
      "cell_type": "markdown",
      "metadata": {},
      "source": ["# Title\n", "Intro paragraph.\n"]
    },
    {
      "cell_type": "code",
      "execution_count": 3,
      "metadata": {},
      "source": ["print('hi')\n"],
      "outputs": [
        {
          "output_type": "stream",
          "name": "stdout",
          "text": ["hi\n"]
        }
      ]
    },
    {
      "cell_type": "code",
      "execution_count": 4,
      "metadata": {},
      "source": ["1 + 1\n"],
      "outputs": [
        {
          "output_type": "execute_result",
          "data": {"text/plain": ["2"]},
          "metadata": {},
          "execution_count": 4
        }
      ]
    }
  ]
}"##
    }

    fn context(root: &std::path::Path) -> ToolExecutionContext {
        ToolExecutionContext {
            workspace_root: root.to_path_buf(),
            workspace_only: true,
            ..Default::default()
        }
    }

    bounded_async_test!(
        async fn notebook_read_returns_typed_cells_and_metadata() {
            let dir = tempfile::tempdir().unwrap();
            let notebook_path = dir.path().join("sample.ipynb");
            std::fs::write(&notebook_path, sample_notebook()).unwrap();

            let result = NotebookReadTool::new()
                .execute(
                    ToolCallId::new(),
                    serde_json::to_value(NotebookReadToolInput {
                        path: "sample.ipynb".to_string(),
                        start_cell: None,
                        end_cell: None,
                        cell_count: Some(2),
                        include_outputs: Some(true),
                    })
                    .unwrap(),
                    &context(dir.path()),
                )
                .await
                .unwrap();

            assert!(!result.is_error);
            let structured = result.structured_content.unwrap();
            assert_eq!(
                structured.get("total_cells").and_then(Value::as_u64),
                Some(3)
            );
            assert_eq!(
                structured.get("output_cells").and_then(Value::as_u64),
                Some(2)
            );
            assert_eq!(
                structured.get("language_name").and_then(Value::as_str),
                Some("python")
            );
            assert_eq!(
                structured.get("kernelspec_name").and_then(Value::as_str),
                Some("python3")
            );
            let cells = structured.get("cells").and_then(Value::as_array).unwrap();
            assert_eq!(cells.len(), 2);
            assert_eq!(
                cells[0].get("cell_type").and_then(Value::as_str),
                Some("markdown")
            );
            assert_eq!(
                cells[1].get("cell_type").and_then(Value::as_str),
                Some("code")
            );

            let MessagePart::Text { text } = &result.parts[0] else {
                panic!("expected text output");
            };
            assert!(text.contains("Cell 1 [markdown]"));
            assert!(text.contains("[output stream] stdout"));
        }
    );

    bounded_async_test!(
        async fn notebook_read_paginates_cells_with_next_start_cell() {
            let dir = tempfile::tempdir().unwrap();
            let notebook_path = dir.path().join("sample.ipynb");
            std::fs::write(&notebook_path, sample_notebook()).unwrap();

            let result = NotebookReadTool::new()
                .execute(
                    ToolCallId::new(),
                    serde_json::to_value(NotebookReadToolInput {
                        path: "sample.ipynb".to_string(),
                        start_cell: Some(2),
                        end_cell: None,
                        cell_count: Some(1),
                        include_outputs: Some(false),
                    })
                    .unwrap(),
                    &context(dir.path()),
                )
                .await
                .unwrap();

            let structured = result.structured_content.unwrap();
            assert_eq!(
                structured.get("start_cell").and_then(Value::as_u64),
                Some(2)
            );
            assert_eq!(structured.get("end_cell").and_then(Value::as_u64), Some(2));
            assert_eq!(
                structured.get("next_start_cell").and_then(Value::as_u64),
                Some(3)
            );
            let cells = structured.get("cells").and_then(Value::as_array).unwrap();
            assert_eq!(cells.len(), 1);
            assert_eq!(
                cells[0].get("output_count").and_then(Value::as_u64),
                Some(1)
            );
            assert!(cells[0].get("outputs").is_none());
        }
    );

    bounded_async_test!(
        async fn notebook_read_rejects_non_notebook_paths() {
            let dir = tempfile::tempdir().unwrap();
            let notebook_path = dir.path().join("sample.txt");
            std::fs::write(&notebook_path, sample_notebook()).unwrap();

            let err = NotebookReadTool::new()
                .execute(
                    ToolCallId::new(),
                    serde_json::to_value(NotebookReadToolInput {
                        path: "sample.txt".to_string(),
                        start_cell: None,
                        end_cell: None,
                        cell_count: None,
                        include_outputs: None,
                    })
                    .unwrap(),
                    &context(dir.path()),
                )
                .await
                .unwrap_err();

            assert!(err.to_string().contains(".ipynb"));
        }
    );

    bounded_async_test!(
        async fn notebook_edit_applies_typed_operations_and_resets_code_outputs() {
            let dir = tempfile::tempdir().unwrap();
            let notebook_path = dir.path().join("sample.ipynb");
            std::fs::write(&notebook_path, sample_notebook()).unwrap();

            let result = NotebookEditTool::new()
                .execute(
                    ToolCallId::new(),
                    serde_json::to_value(NotebookEditToolInput {
                        path: "sample.ipynb".to_string(),
                        operations: vec![
                            NotebookEditOperation::ReplaceCell {
                                cell_index: 2,
                                cell_type: NotebookEditableCellType::Code,
                                source: "print('updated')\n".to_string(),
                            },
                            NotebookEditOperation::InsertCell {
                                cell_index: 3,
                                cell_type: NotebookEditableCellType::Markdown,
                                source: "## Added\n".to_string(),
                            },
                        ],
                        expected_snapshot: None,
                    })
                    .unwrap(),
                    &context(dir.path()),
                )
                .await
                .unwrap();

            assert!(!result.is_error);
            let structured = result.structured_content.unwrap();
            assert_eq!(
                structured.get("kind").and_then(Value::as_str),
                Some("success")
            );
            assert_eq!(
                structured.get("cell_count_before").and_then(Value::as_u64),
                Some(3)
            );
            assert_eq!(
                structured.get("cell_count_after").and_then(Value::as_u64),
                Some(4)
            );
            let operations = structured
                .get("operations")
                .and_then(Value::as_array)
                .expect("operations");
            assert_eq!(operations.len(), 2);
            let changed_cells = structured
                .get("changed_cells")
                .and_then(Value::as_array)
                .expect("changed cells");
            assert_eq!(changed_cells.len(), 2);

            let written: Value =
                serde_json::from_str(&std::fs::read_to_string(&notebook_path).unwrap()).unwrap();
            let cells = written.get("cells").and_then(Value::as_array).unwrap();
            assert_eq!(cells.len(), 4);
            assert_eq!(
                cells[1].get("cell_type").and_then(Value::as_str),
                Some("code")
            );
            assert_eq!(cells[1].get("execution_count"), Some(&Value::Null));
            assert_eq!(
                cells[1]
                    .get("outputs")
                    .and_then(Value::as_array)
                    .map(Vec::len),
                Some(0)
            );
            assert_eq!(
                cells[2].get("cell_type").and_then(Value::as_str),
                Some("markdown")
            );
            let MessagePart::Text { text } = &result.parts[0] else {
                panic!("expected text output");
            };
            assert!(text.contains("Updated sample.ipynb with 2 notebook operation(s)"));
            assert!(text.contains("replaced cell 2 as code"));
            assert!(text.contains("inserted markdown cell at 3"));
        }
    );

    bounded_async_test!(
        async fn notebook_edit_rejects_stale_snapshot_guards() {
            let dir = tempfile::tempdir().unwrap();
            let notebook_path = dir.path().join("sample.ipynb");
            std::fs::write(&notebook_path, sample_notebook()).unwrap();

            let result = NotebookEditTool::new()
                .execute(
                    ToolCallId::new(),
                    serde_json::to_value(NotebookEditToolInput {
                        path: "sample.ipynb".to_string(),
                        operations: vec![NotebookEditOperation::DeleteCell { cell_index: 1 }],
                        expected_snapshot: Some("stale-snapshot".to_string()),
                    })
                    .unwrap(),
                    &context(dir.path()),
                )
                .await
                .unwrap();

            assert!(result.is_error);
            let structured = result.structured_content.unwrap();
            assert_eq!(
                structured.get("kind").and_then(Value::as_str),
                Some("error")
            );
            assert_eq!(
                structured.get("operation_count").and_then(Value::as_u64),
                Some(1)
            );
            assert!(
                structured
                    .get("summary")
                    .and_then(Value::as_str)
                    .is_some_and(|summary| summary.contains("Snapshot mismatch"))
            );

            let written = std::fs::read_to_string(&notebook_path).unwrap();
            assert_eq!(written, sample_notebook());
        }
    );
}
