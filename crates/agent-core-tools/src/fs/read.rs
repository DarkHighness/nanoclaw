use crate::ToolExecutionContext;
use crate::annotations::mcp_tool_annotations;
use crate::fs::{TextBuffer, format_numbered_lines, stable_text_hash};
use crate::fs::{assert_path_inside_root, resolve_tool_path_against_workspace_root};
use crate::registry::Tool;
use agent_core_types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};
use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use base64::Engine;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::fs;

const DEFAULT_READ_PAGE_MAX_BYTES: usize = 50 * 1024;
const MAX_ADAPTIVE_READ_MAX_BYTES: usize = 512 * 1024;
const ADAPTIVE_READ_CONTEXT_SHARE: f64 = 0.2;
const CHARS_PER_TOKEN_ESTIMATE: usize = 4;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct ReadToolInput {
    pub path: String,
    #[serde(default, alias = "offset")]
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
    #[serde(default, alias = "limit")]
    pub line_count: Option<usize>,
    pub annotate_lines: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct ReadTool;

impl ReadTool {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read".to_string(),
            description: "Read a file or image. Text files are returned as a line-numbered view with start_line/end_line or line_count paging, plus snapshot ids for follow-up edits.".to_string(),
            input_schema: serde_json::to_value(schema_for!(ReadToolInput)).expect("read schema"),
            output_mode: ToolOutputMode::ContentParts,
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
        let external_call_id = call_id.0.clone();
        let input: ReadToolInput = serde_json::from_value(arguments)?;
        let resolved = resolve_tool_path_against_workspace_root(
            &input.path,
            ctx.effective_root(),
            ctx.container_workdir.as_deref(),
        )?;
        if ctx.workspace_only {
            assert_path_inside_root(&resolved, ctx.effective_root())?;
        }
        let bytes = fs::read(&resolved).await?;
        if let Some(mime) = sniff_image_mime(&bytes, &resolved) {
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
                metadata: Some(serde_json::json!({ "path": resolved })),
                is_error: false,
            });
        }

        let text = String::from_utf8(bytes)
            .map_err(|_| anyhow!("read: file is not valid UTF-8 or supported image"))?;
        if input.end_line.is_some() && input.line_count.is_some() {
            bail!("read accepts either end_line or line_count, not both");
        }
        let snapshot_id = stable_text_hash(&text);
        let buffer = TextBuffer::parse(&text);
        let total_lines = buffer.line_count();
        let annotate_lines = input.annotate_lines.unwrap_or(true);
        let start_line = input.start_line.unwrap_or(1).max(1);

        if buffer.is_empty() {
            let output = format!(
                "[read path={} lines=0/0 snapshot={}]\n[File is empty]",
                input.path, snapshot_id
            );
            return Ok(ToolResult {
                id: call_id,
                call_id: external_call_id,
                tool_name: "read".to_string(),
                parts: vec![MessagePart::text(output)],
                metadata: Some(serde_json::json!({
                    "path": resolved,
                    "snapshot_id": snapshot_id,
                    "start_line": 0,
                    "end_line": 0,
                    "total_lines": 0,
                    "remaining_lines": 0,
                    "annotate_lines": annotate_lines,
                })),
                is_error: false,
            });
        }

        if start_line > total_lines {
            bail!(
                "start_line {} is beyond end of file ({} lines total)",
                start_line,
                total_lines
            );
        }

        let budget = resolve_adaptive_read_max_bytes(ctx.model_context_window_tokens);
        let all_lines = buffer.lines();
        let selected = &all_lines[start_line - 1..];

        let mut output_lines = Vec::<String>::new();
        let mut used_bytes = 0usize;
        let mut consumed = 0usize;

        if let Some(end_line) = input.end_line {
            if end_line < start_line {
                bail!("end_line {end_line} is before start_line {start_line}");
            }
            let count = end_line.min(total_lines) - start_line + 1;
            for line in selected.iter().take(count) {
                output_lines.push(line.clone());
                consumed += 1;
            }
        } else if let Some(line_count) = input.line_count {
            if line_count == 0 {
                bail!("line_count must be at least 1");
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
                    return Ok(ToolResult::text(call_id, "read", notice));
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
        let remaining_lines = total_lines.saturating_sub(start_line + consumed - 1);
        if remaining_lines > 0 {
            let next_start_line = start_line + consumed;
            if input.line_count.is_some() || input.end_line.is_some() {
                output.push_str(&format!(
                    "\n\n[{remaining_lines} more lines in file. Use start_line={next_start_line} to continue.]"
                ));
            } else {
                output.push_str(&format!(
                    "\n\n[Showing lines {}-{} of {} ({} limit). Use start_line={} to continue.]",
                    start_line,
                    end_line,
                    total_lines,
                    format_bytes(budget),
                    next_start_line
                ));
            }
        }

        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "read".to_string(),
            parts: vec![MessagePart::text(output)],
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
            })),
            is_error: false,
        })
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
    use agent_core_types::ToolCallId;

    fn context(root: &std::path::Path) -> ToolExecutionContext {
        ToolExecutionContext {
            workspace_root: root.to_path_buf(),
            sandbox_root: None,
            workspace_only: true,
            container_workdir: None,
            model_context_window_tokens: None,
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
    }

    #[tokio::test]
    async fn read_tool_accepts_legacy_offset_and_limit_aliases() {
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
                    "offset": 2,
                    "limit": 1
                }),
                &context(dir.path()),
            )
            .await
            .unwrap();

        let text = result.text_content();
        assert!(text.contains("lines=2-2 / 3"));
        assert!(text.contains(" 2 | beta"));
    }
}
