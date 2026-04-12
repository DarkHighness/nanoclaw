use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::file_activity::FileActivityObserver;
use crate::fs::resolve_tool_path_against_workspace_root;
use crate::registry::Tool;
use crate::{Result, ToolError, ToolExecutionContext};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::fs;
use types::{MessagePart, ToolCallId, ToolOutputMode, ToolResult, ToolSpec};

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

#[derive(Clone, Default)]
pub struct NotebookReadTool {
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

#[cfg(test)]
mod tests {
    use super::{NotebookReadTool, NotebookReadToolInput};
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
}
