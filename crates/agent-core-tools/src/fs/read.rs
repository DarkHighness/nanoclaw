use crate::ToolExecutionContext;
use crate::annotations::mcp_tool_annotations;
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
    pub offset: Option<usize>,
    pub limit: Option<usize>,
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
            description: "Read the contents of a file. Supports text files and images. For text files, use offset and limit for paging.".to_string(),
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
        let lines: Vec<&str> = text.split('\n').collect();
        let start = input.offset.unwrap_or(1).max(1) - 1;
        if start >= lines.len() {
            bail!(
                "Offset {} is beyond end of file ({} lines total)",
                start + 1,
                lines.len()
            );
        }
        let budget = resolve_adaptive_read_max_bytes(ctx.model_context_window_tokens);
        let selected = &lines[start..];

        let mut output_lines = Vec::new();
        let mut used_bytes = 0usize;
        let mut consumed = 0usize;

        if let Some(limit) = input.limit {
            for line in selected.iter().take(limit) {
                output_lines.push((*line).to_string());
                consumed += 1;
            }
        } else {
            for line in selected {
                let line_len = line.len() + usize::from(!output_lines.is_empty());
                if output_lines.is_empty() && line_len > budget {
                    let size = format_bytes(line.len());
                    let limit = format_bytes(budget);
                    let notice = format!(
                        "[Line {} is {size}, exceeds {limit} limit. Use offset={} and a smaller limit to continue.]",
                        start + 1,
                        start + 1
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

        let mut output = output_lines.join("\n");
        let remaining_lines = lines.len().saturating_sub(start + consumed);
        if remaining_lines > 0 {
            let next_offset = start + consumed + 1;
            if input.limit.is_some() {
                output.push_str(&format!(
                    "\n\n[{remaining_lines} more lines in file. Use offset={next_offset} to continue.]"
                ));
            } else {
                let end = start + consumed;
                output.push_str(&format!(
                    "\n\n[Showing lines {}-{} of {} ({} limit). Use offset={} to continue.]",
                    start + 1,
                    end,
                    lines.len(),
                    format_bytes(budget),
                    next_offset
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
                "start_line": start + 1,
                "output_lines": consumed,
                "remaining_lines": remaining_lines,
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
