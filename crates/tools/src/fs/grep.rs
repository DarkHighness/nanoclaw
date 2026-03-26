use crate::annotations::mcp_tool_annotations;
use crate::fs::resolve_tool_path_against_workspace_root;
use crate::registry::Tool;
use crate::{Result, ToolExecutionContext};
use async_trait::async_trait;
use globset::Glob;
use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tokio::fs;
use types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};

const DEFAULT_GREP_LIMIT: usize = 100;
const DEFAULT_GREP_MAX_BYTES: usize = 50 * 1024;
const GREP_MAX_LINE_LENGTH: usize = 400;

#[derive(Clone, Copy, PartialEq, Eq)]
enum LineRole {
    Match,
    Context,
}

impl LineRole {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Match => "match",
            Self::Context => "context",
        }
    }
}

#[derive(Clone)]
// Metadata helper that mirrors a displayed line so callers can reason about match vs context rows.
struct GrepLine {
    text: String,
    path: String,
    line_number: usize,
    role: LineRole,
    trimmed: bool,
    content: String,
}

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
        let requested_path = input.path.as_deref().unwrap_or(".");
        let search_path = resolve_tool_path_against_workspace_root(
            requested_path,
            ctx.effective_root(),
            ctx.container_workdir.as_deref(),
        )?;
        let search_path_string = search_path.to_string_lossy().to_string();
        if ctx.workspace_only {
            ctx.assert_path_allowed(&search_path)?;
        }

        let pattern = input.pattern.clone();
        let literal = input.literal.unwrap_or(false);
        let ignore_case = input.ignore_case.unwrap_or(false);
        let matcher = build_pattern(&pattern, literal, ignore_case)?;
        let glob_pattern = input.glob.clone();
        let glob = glob_pattern
            .as_deref()
            .map(|pattern| Glob::new(pattern).map(|glob| glob.compile_matcher()))
            .transpose()?;
        let limit = input.limit.unwrap_or(DEFAULT_GREP_LIMIT).max(1);
        let context = input.context.unwrap_or(0);
        let mut display_lines = Vec::new();
        let mut observed_matches = 0usize;
        let mut line_truncated = false;
        let mut byte_capped = false;
        let mut limit_reached = false;

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
            let display_path = display_path(&file, &search_path);
            let mut file_display_lines = BTreeMap::<usize, GrepLine>::new();
            for (index, line) in content_lines.iter().enumerate() {
                if !matcher.is_match(line) {
                    continue;
                }
                observed_matches += 1;
                if observed_matches > limit {
                    limit_reached = true;
                    break;
                }
                let start = index.saturating_sub(context);
                let end = (index + context).min(content_lines.len().saturating_sub(1));
                for current in start..=end {
                    let source = content_lines[current].replace('\r', "");
                    let (trimmed, was_truncated) = truncate_line(&source, GREP_MAX_LINE_LENGTH);
                    line_truncated |= was_truncated;
                    let role = if current == index {
                        LineRole::Match
                    } else {
                        LineRole::Context
                    };
                    let line_number = current + 1;
                    let display_text = if role == LineRole::Match {
                        format!("{display_path}:{line_number}: {trimmed}")
                    } else {
                        format!("{display_path}-{line_number}- {trimmed}")
                    };
                    let next_line = GrepLine {
                        text: display_text,
                        path: display_path.clone(),
                        line_number,
                        role,
                        trimmed: was_truncated,
                        content: trimmed.clone(),
                    };
                    match file_display_lines.get_mut(&line_number) {
                        // Overlapping context windows can report the same line more than once.
                        // Keep one row per file+line and upgrade context rows to match rows when needed.
                        Some(existing)
                            if matches!(existing.role, LineRole::Context)
                                && matches!(role, LineRole::Match) =>
                        {
                            *existing = next_line;
                        }
                        Some(_) => {}
                        None => {
                            file_display_lines.insert(line_number, next_line);
                        }
                    }
                }
            }
            display_lines.extend(file_display_lines.into_values());
            if limit_reached {
                break;
            }
        }

        let header = format!(
            "[grep pattern={} path={} context={} limit={} truncated={} byte_limit={} ignore_case={} literal={}]",
            pattern,
            requested_path,
            context,
            limit,
            limit_reached,
            DEFAULT_GREP_MAX_BYTES,
            ignore_case,
            literal
        );
        let mut output = header.clone();
        let mut emitted_lines = Vec::new();
        if display_lines.is_empty() {
            output.push_str("\n[No matches found]");
        } else {
            for line in &display_lines {
                let next = format!("\n{}", line.text);
                if output.len() + next.len() > DEFAULT_GREP_MAX_BYTES {
                    byte_capped = true;
                    break;
                }
                output.push_str(&next);
                emitted_lines.push(line.clone());
            }
        }
        let bytes_output = output.len();

        let mut notices = Vec::new();
        if limit_reached {
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

        // Each entry mirrors a line that made it into the final output for downstream tooling.
        let entries: Vec<Value> = emitted_lines
            .iter()
            .map(|line| {
                serde_json::json!({
                    "path": line.path,
                    "line": line.line_number,
                    "role": line.role.as_str(),
                    "content": line.content,
                    "trimmed": line.trimmed,
                })
            })
            .collect();
        let lines_returned = emitted_lines.len();
        let context_lines = emitted_lines
            .iter()
            .filter(|line| matches!(line.role, LineRole::Context))
            .count();

        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id.into(),
            tool_name: "grep".to_string(),
            parts: vec![MessagePart::text(output)],
            metadata: Some(serde_json::json!({
                "path": search_path_string,
                "requested_path": requested_path,
                "pattern": pattern,
                "glob": glob_pattern,
                "context": context,
                "limit": limit,
                "match_count": observed_matches.min(limit),
                "match_count_lower_bound": observed_matches,
                "lines_collected": display_lines.len(),
                "lines_returned": lines_returned,
                "context_lines": context_lines,
                "limit_reached": limit_reached,
                "byte_capped": byte_capped,
                "line_truncated": line_truncated,
                "bytes_output": bytes_output,
                "byte_limit": DEFAULT_GREP_MAX_BYTES,
                "header": header,
                "ignore_case": ignore_case,
                "literal": literal,
                "entries": entries,
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
    files.sort_unstable();
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

#[cfg(test)]
mod tests {
    use super::{GrepTool, GrepToolInput};
    use crate::{Tool, ToolExecutionContext};
    use std::path::Path;
    use types::ToolCallId;

    fn context(root: &Path) -> ToolExecutionContext {
        ToolExecutionContext {
            workspace_root: root.to_path_buf(),
            workspace_only: true,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn grep_tool_reports_metadata_with_matches() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("sample.txt"), "alpha\nbeta\ngamma\n")
            .await
            .unwrap();

        let result = GrepTool::new()
            .execute(
                ToolCallId::new(),
                serde_json::to_value(GrepToolInput {
                    pattern: "beta".to_string(),
                    path: None,
                    glob: None,
                    ignore_case: None,
                    literal: None,
                    context: Some(1),
                    limit: None,
                })
                .unwrap(),
                &context(dir.path()),
            )
            .await
            .unwrap();

        let text = result.text_content();
        assert!(text.starts_with("[grep pattern=beta"));
        assert!(text.contains("sample.txt:2: beta"));
        let metadata = result.metadata.unwrap();
        assert_eq!(metadata["match_count"].as_u64().unwrap(), 1);
        assert_eq!(metadata["lines_returned"].as_u64().unwrap(), 3);
        assert!(metadata["header"].as_str().unwrap().contains("context=1"));
        assert!(!metadata["ignore_case"].as_bool().unwrap());
        let entries = metadata["entries"].as_array().unwrap();
        assert!(entries.iter().any(|entry| {
            entry["line"].as_u64().unwrap() == 2 && entry["role"].as_str().unwrap() == "match"
        }));
    }

    #[tokio::test]
    async fn grep_tool_deduplicates_overlapping_context_rows() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(
            dir.path().join("sample.txt"),
            "a\nmatch one\nmatch two\nz\n",
        )
        .await
        .unwrap();

        let result = GrepTool::new()
            .execute(
                ToolCallId::new(),
                serde_json::to_value(GrepToolInput {
                    pattern: "match".to_string(),
                    path: None,
                    glob: None,
                    ignore_case: None,
                    literal: None,
                    context: Some(1),
                    limit: None,
                })
                .unwrap(),
                &context(dir.path()),
            )
            .await
            .unwrap();

        let metadata = result.metadata.unwrap();
        let entries = metadata["entries"].as_array().unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for entry in entries {
            let key = (
                entry["path"].as_str().unwrap().to_string(),
                entry["line"].as_u64().unwrap(),
            );
            assert!(seen.insert(key), "duplicate line emitted in grep context");
        }
    }
}
