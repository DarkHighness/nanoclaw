use crate::ToolExecutionContext;
use crate::annotations::mcp_tool_annotations;
use crate::fs::{
    TextBuffer, format_numbered_lines, resolve_tool_path_against_workspace_root, stable_text_hash,
};
use crate::registry::Tool;
use crate::{Result, ToolError};
use async_trait::async_trait;
use base64::Engine;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::fs;
use types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};

const DEFAULT_READ_PAGE_MAX_BYTES: usize = 50 * 1024;
const MAX_ADAPTIVE_READ_MAX_BYTES: usize = 512 * 1024;
const ADAPTIVE_READ_CONTEXT_SHARE: f64 = 0.2;
const CHARS_PER_TOKEN_ESTIMATE: usize = 4;
const DEFAULT_ANCHOR_CONTEXT: usize = 8;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct ReadToolInput {
    pub path: String,
    #[serde(default)]
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
    #[serde(default)]
    pub line_count: Option<usize>,
    pub annotate_lines: Option<bool>,
    #[serde(default)]
    pub anchor_text: Option<String>,
    #[serde(default)]
    pub anchor_context: Option<usize>,
    #[serde(default)]
    pub anchor_occurrence: Option<usize>,
    #[serde(default)]
    pub anchor_ignore_case: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct ReadTool;

impl ReadTool {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct ReadOutputAnchor {
    text: String,
    context: usize,
    occurrence: usize,
    ignore_case: bool,
    line: Option<usize>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ReadToolOutput {
    Window {
        requested_path: String,
        resolved_path: String,
        snapshot_id: String,
        selection_hash: String,
        start_line: usize,
        end_line: usize,
        output_lines: usize,
        total_lines: usize,
        remaining_lines: usize,
        annotate_lines: bool,
        next_start_line: Option<usize>,
        anchor: Option<ReadOutputAnchor>,
        empty: bool,
    },
    Image {
        requested_path: String,
        resolved_path: String,
        mime_type: String,
        byte_length: usize,
    },
    Notice {
        requested_path: String,
        resolved_path: String,
        snapshot_id: String,
        start_line: usize,
        total_lines: usize,
        max_bytes: usize,
        message: String,
    },
}

#[async_trait]
impl Tool for ReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read".to_string(),
            description: "Read a file or image. Text files are returned as a line-numbered view with range paging (`start_line`/`end_line` or `line_count`) or anchor-based spans (`anchor_text`), plus snapshot ids for follow-up edits.".to_string(),
            input_schema: serde_json::to_value(schema_for!(ReadToolInput)).expect("read schema"),
            output_mode: ToolOutputMode::ContentParts,
            output_schema: Some(
                serde_json::to_value(schema_for!(ReadToolOutput)).expect("read output schema"),
            ),
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("Read File", true, false, true, false),
        }
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: ReadToolInput = serde_json::from_value(arguments)?;
        let resolved = resolve_tool_path_against_workspace_root(
            &input.path,
            ctx.effective_root(),
            ctx.container_workdir.as_deref(),
        )?;
        if ctx.workspace_only {
            ctx.assert_path_allowed(&resolved)?;
        }
        let bytes = fs::read(&resolved).await?;
        if let Some(mime) = sniff_image_mime(&bytes, &resolved) {
            let byte_length = bytes.len();
            return Ok(ToolResult {
                id: call_id,
                call_id: external_call_id.clone(),
                tool_name: "read".to_string(),
                parts: vec![
                    MessagePart::text(format!("Read image file [{mime}]")),
                    MessagePart::Image {
                        mime_type: mime.to_string(),
                        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                    },
                ],
                structured_content: Some(
                    serde_json::to_value(ReadToolOutput::Image {
                        requested_path: input.path.clone(),
                        resolved_path: resolved.display().to_string(),
                        mime_type: mime.to_string(),
                        byte_length,
                    })
                    .expect("read image output"),
                ),
                metadata: Some(serde_json::json!({ "path": resolved })),
                is_error: false,
            });
        }

        let text = String::from_utf8(bytes)
            .map_err(|_| ToolError::invalid("read: file is not valid UTF-8 or supported image"))?;
        if input.end_line.is_some() && input.line_count.is_some() {
            return Err(ToolError::invalid(
                "read accepts either end_line or line_count, not both",
            ));
        }
        let snapshot_id = stable_text_hash(&text);
        let buffer = TextBuffer::parse(&text);
        let total_lines = buffer.line_count();
        let annotate_lines = input.annotate_lines.unwrap_or(true);
        let mut anchor_line = None;
        let mut forced_end_line = None;
        let mut start_line = input.start_line.unwrap_or(1).max(1);

        if buffer.is_empty() {
            let output = format!(
                "[read path={} lines=0/0 snapshot={}]\n[File is empty]",
                input.path, snapshot_id
            );
            let structured_output = ReadToolOutput::Window {
                requested_path: input.path.clone(),
                resolved_path: resolved.display().to_string(),
                snapshot_id: snapshot_id.clone(),
                selection_hash: stable_text_hash(""),
                start_line: 0,
                end_line: 0,
                output_lines: 0,
                total_lines: 0,
                remaining_lines: 0,
                annotate_lines,
                next_start_line: None,
                anchor: None,
                empty: true,
            };
            return Ok(ToolResult {
                id: call_id,
                call_id: external_call_id,
                tool_name: "read".to_string(),
                parts: vec![MessagePart::text(output)],
                structured_content: Some(
                    serde_json::to_value(structured_output).expect("read empty output"),
                ),
                metadata: Some(serde_json::json!({
                    "path": resolved,
                    "snapshot_id": snapshot_id,
                    "start_line": 0,
                    "end_line": 0,
                    "total_lines": 0,
                    "remaining_lines": 0,
                    "annotate_lines": annotate_lines,
                    "anchor": Value::Null,
                })),
                is_error: false,
            });
        }

        if let Some(anchor_text) = input.anchor_text.as_deref() {
            if input.start_line.is_some() || input.end_line.is_some() || input.line_count.is_some()
            {
                return Err(ToolError::invalid(
                    "anchor_text cannot be combined with start_line, end_line, or line_count",
                ));
            }
            let occurrence = input.anchor_occurrence.unwrap_or(1);
            if occurrence == 0 {
                return Err(ToolError::invalid("anchor_occurrence must be at least 1"));
            }
            let context_lines = input.anchor_context.unwrap_or(DEFAULT_ANCHOR_CONTEXT);
            let ignore_case = input.anchor_ignore_case.unwrap_or(false);
            let (resolved_start_line, resolved_end_line, matched_line) = resolve_anchor_window(
                &buffer,
                anchor_text,
                occurrence,
                context_lines,
                ignore_case,
            )?;
            start_line = resolved_start_line;
            forced_end_line = Some(resolved_end_line);
            anchor_line = Some(matched_line);
        }

        if start_line > total_lines {
            return Err(ToolError::invalid(format!(
                "start_line {} is beyond end of file ({} lines total)",
                start_line, total_lines
            )));
        }

        let budget = resolve_adaptive_read_max_bytes(ctx.model_context_window_tokens);
        let all_lines = buffer.lines();
        let selected = &all_lines[start_line - 1..];

        let mut output_lines = Vec::<String>::new();
        let mut used_bytes = 0usize;
        let mut consumed = 0usize;

        if let Some(anchor_end_line) = forced_end_line {
            let count = anchor_end_line - start_line + 1;
            for line in selected.iter().take(count) {
                output_lines.push(line.clone());
                consumed += 1;
            }
        } else if let Some(end_line) = input.end_line {
            if end_line < start_line {
                return Err(ToolError::invalid(format!(
                    "end_line {end_line} is before start_line {start_line}"
                )));
            }
            let count = end_line.min(total_lines) - start_line + 1;
            for line in selected.iter().take(count) {
                output_lines.push(line.clone());
                consumed += 1;
            }
        } else if let Some(line_count) = input.line_count {
            if line_count == 0 {
                return Err(ToolError::invalid("line_count must be at least 1"));
            }
            for line in selected.iter().take(line_count) {
                output_lines.push(line.clone());
                consumed += 1;
            }
        } else {
            for line in selected {
                let line_len = line.len() + usize::from(!output_lines.is_empty());
                if output_lines.is_empty() && line_len > budget {
                    let size = format_bytes(line.len());
                    let limit = format_bytes(budget);
                    let notice = format!(
                        "[Line {} is {size}, exceeds {limit} limit. Use start_line={} with a smaller line_count to continue.]",
                        start_line, start_line
                    );
                    return Ok(ToolResult {
                        id: call_id,
                        call_id: external_call_id,
                        tool_name: "read".to_string(),
                        parts: vec![MessagePart::text(notice.clone())],
                        structured_content: Some(
                            serde_json::to_value(ReadToolOutput::Notice {
                                requested_path: input.path.clone(),
                                resolved_path: resolved.display().to_string(),
                                snapshot_id: snapshot_id.clone(),
                                start_line,
                                total_lines,
                                max_bytes: budget,
                                message: notice,
                            })
                            .expect("read notice output"),
                        ),
                        metadata: Some(serde_json::json!({
                            "path": resolved,
                            "snapshot_id": snapshot_id,
                            "start_line": start_line,
                            "total_lines": total_lines,
                            "byte_limit": budget,
                        })),
                        is_error: false,
                    });
                }
                if used_bytes + line_len > budget {
                    break;
                }
                used_bytes += line_len;
                output_lines.push((*line).to_string());
                consumed += 1;
            }
        }

        let end_line = start_line + consumed.saturating_sub(1);
        let selection_hash = stable_text_hash(&output_lines.join("\n"));
        let header = format!(
            "[read path={} lines={}-{} / {} snapshot={} slice={}]",
            input.path, start_line, end_line, total_lines, snapshot_id, selection_hash
        );
        let body = if annotate_lines {
            format_numbered_lines(&output_lines, start_line)
        } else {
            output_lines.join("\n")
        };
        let mut output = if body.is_empty() {
            header
        } else {
            format!("{header}\n{body}")
        };
        if let Some(anchor_line) = anchor_line {
            output.push_str(&format!(
                "\n\n[anchor matched at line {anchor_line}; use start_line={anchor_line} for a centered follow-up read]"
            ));
        }
        let remaining_lines = total_lines.saturating_sub(start_line + consumed - 1);
        let next_start_line = (remaining_lines > 0).then_some(start_line + consumed);
        if remaining_lines > 0 {
            if input.line_count.is_some() || input.end_line.is_some() || forced_end_line.is_some() {
                output.push_str(&format!(
                    "\n\n[{remaining_lines} more lines in file. Use start_line={} to continue.]",
                    next_start_line.expect("next start line")
                ));
            } else {
                output.push_str(&format!(
                    "\n\n[Showing lines {}-{} of {} ({} limit). Use start_line={} to continue.]",
                    start_line,
                    end_line,
                    total_lines,
                    format_bytes(budget),
                    next_start_line.expect("next start line")
                ));
            }
        }
        let anchor = input.anchor_text.as_ref().map(|value| ReadOutputAnchor {
            text: value.clone(),
            context: input.anchor_context.unwrap_or(DEFAULT_ANCHOR_CONTEXT),
            occurrence: input.anchor_occurrence.unwrap_or(1),
            ignore_case: input.anchor_ignore_case.unwrap_or(false),
            line: anchor_line,
        });
        let structured_output = ReadToolOutput::Window {
            requested_path: input.path.clone(),
            resolved_path: resolved.display().to_string(),
            snapshot_id: snapshot_id.clone(),
            selection_hash: selection_hash.clone(),
            start_line,
            end_line,
            output_lines: consumed,
            total_lines,
            remaining_lines,
            annotate_lines,
            next_start_line,
            anchor: anchor.clone(),
            empty: false,
        };

        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "read".to_string(),
            parts: vec![MessagePart::text(output)],
            // The text or image body stays in `parts`; structured content carries
            // the stable window anchors that follow-up edits and pagination rely on.
            structured_content: Some(
                serde_json::to_value(structured_output).expect("read window output"),
            ),
            metadata: Some(serde_json::json!({
                "path": resolved,
                "snapshot_id": snapshot_id,
                "selection_hash": selection_hash,
                "start_line": start_line,
                "end_line": end_line,
                "output_lines": consumed,
                "total_lines": total_lines,
                "remaining_lines": remaining_lines,
                "annotate_lines": annotate_lines,
                "next_start_line": next_start_line,
                "anchor": anchor.map(|value| serde_json::to_value(value).expect("read anchor metadata")),
            })),
            is_error: false,
        })
    }
}

fn resolve_anchor_window(
    buffer: &TextBuffer,
    anchor_text: &str,
    occurrence: usize,
    context_lines: usize,
    ignore_case: bool,
) -> Result<(usize, usize, usize)> {
    let lines = buffer.lines();
    let mut seen = 0usize;
    for (index, line) in lines.iter().enumerate() {
        if line_contains_anchor(line, anchor_text, ignore_case) {
            seen += 1;
            if seen == occurrence {
                let anchor_line = index + 1;
                let start_line = anchor_line.saturating_sub(context_lines).max(1);
                let end_line = (anchor_line + context_lines).min(lines.len());
                return Ok((start_line, end_line, anchor_line));
            }
        }
    }
    Err(ToolError::invalid(format!(
        "anchor_text was not found at occurrence {} in the selected file",
        occurrence
    )))
}

fn line_contains_anchor(line: &str, anchor_text: &str, ignore_case: bool) -> bool {
    if ignore_case {
        line.to_lowercase().contains(&anchor_text.to_lowercase())
    } else {
        line.contains(anchor_text)
    }
}

#[must_use]
pub fn resolve_adaptive_read_max_bytes(context_window_tokens: Option<usize>) -> usize {
    let Some(tokens) = context_window_tokens else {
        return DEFAULT_READ_PAGE_MAX_BYTES;
    };
    let from_context =
        (tokens as f64 * CHARS_PER_TOKEN_ESTIMATE as f64 * ADAPTIVE_READ_CONTEXT_SHARE) as usize;
    from_context.clamp(DEFAULT_READ_PAGE_MAX_BYTES, MAX_ADAPTIVE_READ_MAX_BYTES)
}

fn sniff_image_mime(bytes: &[u8], path: &std::path::Path) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    match path.extension().and_then(|value| value.to_str()) {
        Some("png") => Some("image/png"),
        Some("jpg") | Some("jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        _ => None,
    }
}

fn format_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{}KB", (bytes as f64 / 1024.0).round() as usize)
    } else {
        format!("{bytes}B")
    }
}

#[cfg(test)]
mod tests {
    use super::{ReadTool, ReadToolInput};
    use crate::{Tool, ToolExecutionContext};
    use types::ToolCallId;

    fn context(root: &std::path::Path) -> ToolExecutionContext {
        ToolExecutionContext {
            workspace_root: root.to_path_buf(),
            workspace_only: true,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn read_tool_returns_line_numbered_view_with_hashes() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("sample.txt"), "alpha\nbeta\ngamma\n")
            .await
            .unwrap();

        let tool = ReadTool::new();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(ReadToolInput {
                    path: "sample.txt".to_string(),
                    start_line: Some(2),
                    end_line: Some(3),
                    line_count: None,
                    annotate_lines: None,
                    anchor_text: None,
                    anchor_context: None,
                    anchor_occurrence: None,
                    anchor_ignore_case: None,
                })
                .unwrap(),
                &context(dir.path()),
            )
            .await
            .unwrap();

        let text = result.text_content();
        assert!(text.contains("[read path=sample.txt lines=2-3 / 3 snapshot="));
        assert!(text.contains(" 2 | beta"));
        assert!(text.contains(" 3 | gamma"));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["kind"], "window");
        assert!(structured["selection_hash"].as_str().unwrap().len() >= 12);
        assert_eq!(structured["next_start_line"], serde_json::json!(null));
    }

    #[tokio::test]
    async fn read_tool_reads_with_explicit_start_line_and_line_count() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("sample.txt"), "alpha\nbeta\ngamma\n")
            .await
            .unwrap();

        let tool = ReadTool::new();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::json!({
                    "path": "sample.txt",
                    "start_line": 2,
                    "line_count": 1
                }),
                &context(dir.path()),
            )
            .await
            .unwrap();

        let text = result.text_content();
        assert!(text.contains("lines=2-2 / 3"));
        assert!(text.contains(" 2 | beta"));
    }

    #[tokio::test]
    async fn read_tool_can_anchor_on_symbol_like_text() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(
            dir.path().join("sample.rs"),
            "fn alpha() {}\nfn beta() {}\nfn target_symbol() {}\nfn gamma() {}\n",
        )
        .await
        .unwrap();

        let result = ReadTool::new()
            .execute(
                ToolCallId::new(),
                serde_json::json!({
                    "path": "sample.rs",
                    "anchor_text": "target_symbol",
                    "anchor_context": 1
                }),
                &context(dir.path()),
            )
            .await
            .unwrap();

        let text = result.text_content();
        assert!(text.contains("lines=2-4 / 4"));
        assert!(text.contains(" 3 | fn target_symbol() {}"));
        let structured = result.structured_content.clone().unwrap();
        assert_eq!(structured["anchor"]["line"], 3);
        let metadata = result.metadata.unwrap();
        assert_eq!(metadata["anchor"]["line"].as_u64().unwrap(), 3);
    }

    #[tokio::test]
    async fn read_tool_rejects_anchor_with_manual_range() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("sample.txt"), "alpha\nbeta\n")
            .await
            .unwrap();

        let err = ReadTool::new()
            .execute(
                ToolCallId::new(),
                serde_json::json!({
                    "path": "sample.txt",
                    "anchor_text": "beta",
                    "start_line": 2
                }),
                &context(dir.path()),
            )
            .await
            .unwrap_err();

        assert!(err.to_string().contains("anchor_text cannot be combined"));
    }
}
