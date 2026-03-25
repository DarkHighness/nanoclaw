use crate::ToolExecutionContext;
use crate::annotations::mcp_tool_annotations;
use crate::fs::{assert_path_inside_root, resolve_tool_path_against_workspace_root};
use crate::registry::Tool;
use agent_core_types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};
use anyhow::Result;
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_MAX_OUTPUT_CHARS: usize = 32 * 1024;
const MAX_ALLOWED_OUTPUT_CHARS: usize = 256 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct BashToolInput {
    pub command: String,
    pub cwd: Option<String>,
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub max_output_chars: Option<usize>,
    #[serde(default)]
    pub env: Option<BTreeMap<String, String>>,
}

#[derive(Clone, Debug, Default)]
pub struct BashTool;

#[derive(Clone, Debug)]
struct OutputSlice {
    text: String,
    truncated: bool,
    original_chars: usize,
}

impl BashTool {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for BashTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "bash".to_string(),
            description: "Run a shell command inside the workspace and capture stdout, stderr, exit status, timeout information, and truncation metadata.".to_string(),
            input_schema: serde_json::to_value(schema_for!(BashToolInput)).expect("bash schema"),
            output_mode: ToolOutputMode::Text,
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("Run Shell Command", false, true, false, true),
        }
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = call_id.0.clone();
        let input: BashToolInput = serde_json::from_value(arguments)?;
        let cwd = resolve_cwd(&input, ctx)?;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let max_output_chars = input
            .max_output_chars
            .unwrap_or(DEFAULT_MAX_OUTPUT_CHARS)
            .clamp(1, MAX_ALLOWED_OUTPUT_CHARS);
        let timeout_ms = input.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS).max(1);

        let mut child = Command::new(&shell);
        child
            .arg("-lc")
            .arg(&input.command)
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(env) = &input.env {
            child.envs(env);
        }

        let future = child.output();
        let output = match timeout(Duration::from_millis(timeout_ms), future).await {
            Ok(result) => result?,
            Err(_) => {
                return Ok(ToolResult {
                    id: call_id,
                    call_id: external_call_id,
                    tool_name: "bash".to_string(),
                    parts: vec![MessagePart::text(format!(
                        "[bash cwd={} timeout_ms={}]\nCommand timed out after {timeout_ms}ms.\ncommand> {}",
                        cwd.display(),
                        timeout_ms,
                        input.command
                    ))],
                    metadata: Some(serde_json::json!({
                        "cwd": cwd,
                        "shell": shell,
                        "command": input.command,
                        "timeout_ms": timeout_ms,
                        "timed_out": true,
                    })),
                    is_error: true,
                });
            }
        };

        let stdout = truncate_output(&String::from_utf8_lossy(&output.stdout), max_output_chars);
        let stderr = truncate_output(&String::from_utf8_lossy(&output.stderr), max_output_chars);
        let exit_code = output.status.code().unwrap_or(-1);
        let text = render_output(
            &input.command,
            &cwd,
            exit_code,
            timeout_ms,
            &stdout,
            &stderr,
        );

        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "bash".to_string(),
            parts: vec![MessagePart::text(text)],
            metadata: Some(serde_json::json!({
                "cwd": cwd,
                "shell": shell,
                "command": input.command,
                "timeout_ms": timeout_ms,
                "exit_code": exit_code,
                "timed_out": false,
                "max_output_chars": max_output_chars,
                "stdout": {
                    "chars": stdout.original_chars,
                    "truncated": stdout.truncated,
                },
                "stderr": {
                    "chars": stderr.original_chars,
                    "truncated": stderr.truncated,
                },
                "env": input.env.as_ref().map(|env| env.keys().cloned().collect::<Vec<_>>()),
            })),
            is_error: !output.status.success(),
        })
    }
}

fn resolve_cwd(input: &BashToolInput, ctx: &ToolExecutionContext) -> Result<PathBuf> {
    let cwd = resolve_tool_path_against_workspace_root(
        input.cwd.as_deref().unwrap_or("."),
        ctx.effective_root(),
        ctx.container_workdir.as_deref(),
    )?;
    if ctx.workspace_only {
        assert_path_inside_root(&cwd, ctx.effective_root())?;
    }
    Ok(cwd)
}

fn truncate_output(output: &str, limit: usize) -> OutputSlice {
    let trimmed = output.trim();
    let original_chars = trimmed.chars().count();
    if original_chars <= limit {
        return OutputSlice {
            text: trimmed.to_string(),
            truncated: false,
            original_chars,
        };
    }
    let prefix = trimmed.chars().take(limit).collect::<String>();
    OutputSlice {
        text: format!("{prefix}\n...[truncated]"),
        truncated: true,
        original_chars,
    }
}

fn render_output(
    command: &str,
    cwd: &std::path::Path,
    exit_code: i32,
    timeout_ms: u64,
    stdout: &OutputSlice,
    stderr: &OutputSlice,
) -> String {
    let mut sections = vec![
        format!(
            "[bash cwd={} exit_code={} timeout_ms={}]",
            cwd.display(),
            exit_code,
            timeout_ms
        ),
        format!("command> {command}"),
    ];
    if !stdout.text.is_empty() {
        sections.push(format!("stdout>\n{}", stdout.text));
    }
    if !stderr.text.is_empty() {
        sections.push(format!("stderr>\n{}", stderr.text));
    }
    if stdout.truncated || stderr.truncated {
        sections.push(format!(
            "[output truncated to {} chars per stream]",
            stdout
                .original_chars
                .max(stderr.original_chars)
                .min(MAX_ALLOWED_OUTPUT_CHARS)
        ));
    }
    sections.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::{BashTool, BashToolInput};
    use crate::{Tool, ToolExecutionContext};
    use agent_core_types::ToolCallId;
    use std::collections::BTreeMap;

    #[tokio::test]
    async fn bash_tool_captures_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let tool = BashTool::new();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(BashToolInput {
                    command: "printf hello".to_string(),
                    cwd: None,
                    timeout_ms: Some(5_000),
                    max_output_chars: None,
                    env: None,
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
        assert!(result.text_content().contains("stdout>\nhello"));
    }

    #[tokio::test]
    async fn bash_tool_can_inject_env_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let tool = BashTool::new();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(BashToolInput {
                    command: "printf %s \"$PATCH_ENV\"".to_string(),
                    cwd: None,
                    timeout_ms: Some(5_000),
                    max_output_chars: None,
                    env: Some(BTreeMap::from([(
                        "PATCH_ENV".to_string(),
                        "value".to_string(),
                    )])),
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
        assert!(result.text_content().contains("stdout>\nvalue"));
    }
}
