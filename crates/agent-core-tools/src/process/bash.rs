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
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_OUTPUT_CHARS: usize = 32 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct BashToolInput {
    pub command: String,
    pub cwd: Option<String>,
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct BashTool;

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
            description: "Run a shell command inside the workspace and capture stdout, stderr, exit status, and timeout information.".to_string(),
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
        let mut child = Command::new(&shell);
        child
            .arg("-lc")
            .arg(&input.command)
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let timeout_ms = input.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS).max(1);
        let future = child.output();
        let output = match timeout(Duration::from_millis(timeout_ms), future).await {
            Ok(result) => result?,
            Err(_) => {
                return Ok(ToolResult::error(
                    call_id,
                    "bash",
                    format!(
                        "Command timed out after {timeout_ms}ms in {}",
                        cwd.display()
                    ),
                ));
            }
        };

        let stdout = truncate_output(&String::from_utf8_lossy(&output.stdout));
        let stderr = truncate_output(&String::from_utf8_lossy(&output.stderr));
        let exit_code = output.status.code().unwrap_or(-1);
        let mut sections = vec![
            format!("command> {}", input.command),
            format!("cwd> {}", cwd.display()),
            format!("exit_code> {exit_code}"),
        ];
        if !stdout.is_empty() {
            sections.push(format!("stdout>\n{stdout}"));
        }
        if !stderr.is_empty() {
            sections.push(format!("stderr>\n{stderr}"));
        }
        let text = sections.join("\n\n");

        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "bash".to_string(),
            parts: vec![MessagePart::text(text)],
            metadata: Some(serde_json::json!({
                "cwd": cwd,
                "exit_code": exit_code,
                "timed_out": false,
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

fn truncate_output(output: &str) -> String {
    let trimmed = output.trim();
    if trimmed.chars().count() <= MAX_OUTPUT_CHARS {
        return trimmed.to_string();
    }
    let prefix = trimmed.chars().take(MAX_OUTPUT_CHARS).collect::<String>();
    format!("{prefix}\n...[truncated]")
}

#[cfg(test)]
mod tests {
    use super::{BashTool, BashToolInput};
    use crate::{Tool, ToolExecutionContext};
    use agent_core_types::ToolCallId;

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
}
