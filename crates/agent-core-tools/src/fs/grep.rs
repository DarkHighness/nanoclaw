use crate::ToolExecutionContext;
use crate::annotations::mcp_tool_annotations;
use crate::fs::{assert_path_inside_root, resolve_tool_path_against_workspace_root};
use crate::registry::Tool;
use agent_core_types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};
use anyhow::Result;
use async_trait::async_trait;
use globset::Glob;
use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use tokio::fs;

const DEFAULT_GREP_LIMIT: usize = 100;
const DEFAULT_GREP_MAX_BYTES: usize = 50 * 1024;
const GREP_MAX_LINE_LENGTH: usize = 400;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct GrepToolInput {
    pub pattern: String,
    pub path: Option<String>,
    pub glob: Option<String>,
    #[serde(rename = "ignoreCase")]
    pub ignore_case: Option<bool>,
    pub literal: Option<bool>,
    pub context: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct GrepTool;

impl GrepTool {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "grep".to_string(),
            description: "Search file contents for a pattern. Returns matching lines with file paths and line numbers.".to_string(),
            input_schema: serde_json::to_value(schema_for!(GrepToolInput)).expect("grep schema"),
            output_mode: ToolOutputMode::Text,
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("Search File Contents", true, false, true, false),
        }
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = call_id.0.clone();
        let input: GrepToolInput = serde_json::from_value(arguments)?;
        let search_path = resolve_tool_path_against_workspace_root(
            input.path.as_deref().unwrap_or("."),
            ctx.effective_root(),
            ctx.container_workdir.as_deref(),
        )?;
        if ctx.workspace_only {
            assert_path_inside_root(&search_path, ctx.effective_root())?;
        }

        let matcher = build_pattern(
            &input.pattern,
            input.literal.unwrap_or(false),
            input.ignore_case.unwrap_or(false),
        )?;
        let glob = input
            .glob
            .as_deref()
            .map(|pattern| Glob::new(pattern).map(|glob| glob.compile_matcher()))
            .transpose()?;
        let limit = input.limit.unwrap_or(DEFAULT_GREP_LIMIT).max(1);
        let context = input.context.unwrap_or(0);
        let mut lines = Vec::new();
        let mut match_count = 0usize;
        let mut line_truncated = false;
        let mut byte_capped = false;

        let files = collect_files(&search_path)?;
        for file in files {
            if let Some(glob_matcher) = &glob {
                let candidate = file.strip_prefix(&search_path).unwrap_or(file.as_path());
                if !glob_matcher.is_match(candidate) && !glob_matcher.is_match(&file) {
                    continue;
                }
            }
            let content = match fs::read_to_string(&file).await {
                Ok(content) => content,
                Err(_) => continue,
            };
            let content_lines: Vec<&str> = content.split('\n').collect();
            for (index, line) in content_lines.iter().enumerate() {
                if !matcher.is_match(line) {
                    continue;
                }
                match_count += 1;
                let display_path = display_path(&file, &search_path);
                let start = index.saturating_sub(context);
                let end = (index + context).min(content_lines.len().saturating_sub(1));
                for current in start..=end {
                    let source = content_lines[current].replace('\r', "");
                    let (trimmed, was_truncated) = truncate_line(&source, GREP_MAX_LINE_LENGTH);
                    line_truncated |= was_truncated;
                    if current == index {
                        lines.push(format!("{display_path}:{}: {trimmed}", current + 1));
                    } else {
                        lines.push(format!("{display_path}-{}- {trimmed}", current + 1));
                    }
                }
                if match_count >= limit {
                    break;
                }
            }
            if match_count >= limit {
                break;
            }
        }

        if lines.is_empty() {
            return Ok(ToolResult::text(call_id, "grep", "No matches found"));
        }

        let mut output = String::new();
        for line in lines {
            let next = if output.is_empty() {
                line
            } else {
                format!("\n{line}")
            };
            if output.len() + next.len() > DEFAULT_GREP_MAX_BYTES {
                byte_capped = true;
                break;
            }
            output.push_str(&next);
        }

        let mut notices = Vec::new();
        if match_count >= limit {
            notices.push(format!(
                "{limit} matches limit reached. Use limit={} for more, or refine pattern",
                limit * 2
            ));
        }
        if byte_capped {
            notices.push(format!(
                "{} limit reached",
                format_bytes(DEFAULT_GREP_MAX_BYTES)
            ));
        }
        if line_truncated {
            notices.push(format!(
                "Some lines truncated to {GREP_MAX_LINE_LENGTH} chars. Use read tool to see full lines"
            ));
        }
        if !notices.is_empty() {
            output.push_str(&format!("\n\n[{}]", notices.join(". ")));
        }

        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "grep".to_string(),
            parts: vec![MessagePart::text(output)],
            metadata: Some(serde_json::json!({
                "pattern": input.pattern,
                "match_count": match_count,
            })),
            is_error: false,
        })
    }
}

fn build_pattern(pattern: &str, literal: bool, ignore_case: bool) -> Result<Regex> {
    let source = if literal {
        regex::escape(pattern)
    } else {
        pattern.to_string()
    };
    Ok(RegexBuilder::new(&source)
        .case_insensitive(ignore_case)
        .build()?)
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
        if entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            files.push(entry.into_path());
        }
    }
    Ok(files)
}

fn display_path(file: &Path, search_path: &Path) -> String {
    if search_path.is_dir() {
        file.strip_prefix(search_path)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/")
    } else {
        file.file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| file.display().to_string())
    }
}

fn truncate_line(line: &str, limit: usize) -> (String, bool) {
    if line.chars().count() <= limit {
        return (line.to_string(), false);
    }
    let truncated = line.chars().take(limit).collect::<String>();
    (format!("{truncated}..."), true)
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
