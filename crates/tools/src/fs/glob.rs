use crate::annotations::mcp_tool_annotations;
use crate::fs::resolve_tool_path_against_workspace_root;
use crate::registry::Tool;
use crate::{Result, ToolExecutionContext};
use async_trait::async_trait;
use globset::Glob;
use ignore::WalkBuilder;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};

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
        let external_call_id = types::CallId::from(&call_id);
        let input: GlobToolInput = serde_json::from_value(arguments)?;
        let requested_path = input.path.as_deref().unwrap_or(".");
        let root = resolve_tool_path_against_workspace_root(
            requested_path,
            ctx.effective_root(),
            ctx.container_workdir.as_deref(),
        )?;
        if ctx.workspace_only {
            ctx.assert_path_allowed(&root)?;
        }

        let pattern = input.pattern.clone();
        let matcher = Glob::new(&pattern)?.compile_matcher();
        let limit = input.limit.unwrap_or(DEFAULT_GLOB_LIMIT).max(1);
        let overflow_limit = limit + 1;
        let mut matches = Vec::new();
        let mut files_scanned = 0usize;

        for file in collect_files(&root)? {
            files_scanned += 1;
            let candidate = file.strip_prefix(&root).unwrap_or(file.as_path());
            if matcher.is_match(candidate) || matcher.is_match(&file) {
                matches.push(display_path(&file, &root));
                if matches.len() >= overflow_limit {
                    break;
                }
            }
        }

        let truncated = matches.len() > limit;
        if truncated {
            matches.truncate(limit);
        }
        let header = format!(
            "[glob pattern={} path={} limit={} truncated={}]",
            pattern, requested_path, limit, truncated
        );
        let mut output_lines = vec![header.clone()];
        if matches.is_empty() {
            output_lines.push("[No matches found]".to_string());
        } else {
            output_lines.extend(matches.iter().cloned());
        }
        if truncated {
            output_lines.push(format!(
                "[{limit} matches limit reached. Narrow the pattern or increase limit.]"
            ));
        }

        // Glob currently only reports files, so the kind metadata is fixed.
        let encoded_matches: Vec<Value> = matches
            .iter()
            .map(|path| {
                serde_json::json!({
                    "path": path,
                    "kind": "file",
                })
            })
            .collect();

        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "glob".to_string(),
            parts: vec![MessagePart::text(output_lines.join("\n"))],
            metadata: Some(serde_json::json!({
                "path": root,
                "requested_path": requested_path,
                "pattern": pattern,
                "limit": limit,
                "files_scanned": files_scanned,
                "match_count": matches.len(),
                "truncated": truncated,
                "header": header,
                "matches": encoded_matches,
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
    files.sort_unstable();
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
    use types::ToolCallId;

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
                    workspace_only: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let output = result.text_content();
        assert!(output.contains("src/lib.rs"));
        assert!(output.contains("src/main.rs"));
        let metadata = result.metadata.unwrap();
        assert_eq!(metadata["match_count"].as_u64().unwrap(), 2);
        assert!(
            metadata["header"]
                .as_str()
                .unwrap()
                .contains("pattern=src/**/*.rs")
        );
        let matches_meta = metadata["matches"].as_array().unwrap();
        assert!(
            matches_meta
                .iter()
                .any(|entry| { entry["path"].as_str().unwrap().ends_with("src/lib.rs") })
        );
    }

    #[tokio::test]
    async fn glob_tool_marks_truncation_only_when_extra_matches_exist() {
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

        let result = GlobTool::new()
            .execute(
                ToolCallId::new(),
                serde_json::to_value(GlobToolInput {
                    pattern: "src/**/*.rs".to_string(),
                    path: None,
                    limit: Some(2),
                })
                .unwrap(),
                &ToolExecutionContext {
                    workspace_root: dir.path().to_path_buf(),
                    workspace_only: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let metadata = result.metadata.unwrap();
        assert!(!metadata["truncated"].as_bool().unwrap());
        assert_eq!(metadata["match_count"].as_u64().unwrap(), 2);
    }
}
