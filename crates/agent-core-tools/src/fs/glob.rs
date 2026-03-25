use crate::ToolExecutionContext;
use crate::annotations::mcp_tool_annotations;
use crate::fs::{assert_path_inside_root, resolve_tool_path_against_workspace_root};
use crate::registry::Tool;
use agent_core_types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};
use anyhow::Result;
use async_trait::async_trait;
use globset::Glob;
use ignore::WalkBuilder;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

const DEFAULT_GLOB_LIMIT: usize = 200;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct GlobToolInput {
    pub pattern: String,
    pub path: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct GlobTool;

impl GlobTool {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GlobTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "glob".to_string(),
            description:
                "Find files under a directory using a glob pattern such as **/*.rs or src/**/*.ts."
                    .to_string(),
            input_schema: serde_json::to_value(schema_for!(GlobToolInput)).expect("glob schema"),
            output_mode: ToolOutputMode::Text,
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("Find Files", true, false, true, false),
        }
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = call_id.0.clone();
        let input: GlobToolInput = serde_json::from_value(arguments)?;
        let root = resolve_tool_path_against_workspace_root(
            input.path.as_deref().unwrap_or("."),
            ctx.effective_root(),
            ctx.container_workdir.as_deref(),
        )?;
        if ctx.workspace_only {
            assert_path_inside_root(&root, ctx.effective_root())?;
        }

        let matcher = Glob::new(&input.pattern)?.compile_matcher();
        let limit = input.limit.unwrap_or(DEFAULT_GLOB_LIMIT).max(1);
        let mut matches = Vec::new();

        for file in collect_files(&root)? {
            let candidate = file.strip_prefix(&root).unwrap_or(file.as_path());
            if matcher.is_match(candidate) || matcher.is_match(&file) {
                matches.push(display_path(&file, &root));
                if matches.len() >= limit {
                    break;
                }
            }
        }

        let mut output = if matches.is_empty() {
            "No matches found".to_string()
        } else {
            matches.join("\n")
        };
        if matches.len() >= limit {
            output.push_str(&format!(
                "\n\n[{limit} matches limit reached. Narrow the pattern or increase limit.]"
            ));
        }

        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "glob".to_string(),
            parts: vec![MessagePart::text(output)],
            metadata: Some(serde_json::json!({
                "pattern": input.pattern,
                "match_count": matches.len(),
            })),
            is_error: false,
        })
    }
}

fn collect_files(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    let mut files = Vec::new();
    let mut builder = WalkBuilder::new(path);
    builder.hidden(false);
    builder.git_ignore(true);
    builder.git_global(true);
    builder.git_exclude(true);
    for entry in builder.build() {
        let entry = entry?;
        if entry.file_type().is_some_and(|value| value.is_file()) {
            files.push(entry.into_path());
        }
    }
    Ok(files)
}

fn display_path(file: &Path, root: &Path) -> String {
    if root.is_dir() {
        file.strip_prefix(root)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/")
    } else {
        file.display().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{GlobTool, GlobToolInput};
    use crate::{Tool, ToolExecutionContext};
    use agent_core_types::ToolCallId;

    #[tokio::test]
    async fn glob_tool_lists_matching_files() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(dir.path().join("src"))
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("src/lib.rs"), "")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("src/main.rs"), "")
            .await
            .unwrap();

        let tool = GlobTool::new();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(GlobToolInput {
                    pattern: "src/**/*.rs".to_string(),
                    path: None,
                    limit: None,
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

        let output = result.text_content();
        assert!(output.contains("src/lib.rs"));
        assert!(output.contains("src/main.rs"));
    }
}
