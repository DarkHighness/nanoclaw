use crate::preview::{PreviewCollapse, collapse_preview_text, command_output_collapse};
use serde_json::Value;

/// Tool timeline entries are still serialized into plain transcript strings, so
/// the shared formatter owns the tree-prefix protocol used by both live and
/// historical views. Keeping that protocol in one place prevents the two
/// surfaces from drifting into different layouts.
pub(crate) fn summarize_tool_entry(
    headline: impl Into<String>,
    detail_lines: Vec<String>,
) -> String {
    let mut lines = vec![headline.into()];
    lines.extend(
        detail_lines
            .into_iter()
            .filter(|line| !line.trim().is_empty()),
    );
    lines.join("\n")
}

pub(crate) fn prefixed_detail_lines(lines: &[String]) -> Vec<String> {
    let mut rendered = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        if index == 0 {
            rendered.push(format!("  └ {line}"));
        } else {
            rendered.push(format!("    {line}"));
        }
    }
    rendered
}

pub(crate) fn tool_arguments_preview_lines(tool_name: &str, arguments: &Value) -> Vec<String> {
    if tool_name == "bash"
        && let Some(command) = arguments.get("command").and_then(Value::as_str)
        && !command.trim().is_empty()
    {
        return collapse_preview_text(
            &format!("$ {}", command.trim()),
            4,
            96,
            PreviewCollapse::Head,
        );
    }

    if tool_name == "update_plan" {
        let item_count = arguments
            .get("plan")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let mut lines = vec![if item_count == 0 {
            "clear plan".to_string()
        } else {
            format!("set {item_count} plan step(s)")
        }];
        if let Some(explanation) = arguments
            .get("explanation")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            lines.extend(collapse_preview_text(
                explanation,
                2,
                96,
                PreviewCollapse::Head,
            ));
        }
        return lines;
    }

    for key in ["path", "uri", "query", "prompt", "message"] {
        if let Some(value) = arguments.get(key).and_then(Value::as_str)
            && !value.trim().is_empty()
        {
            return collapse_preview_text(value.trim(), 4, 96, PreviewCollapse::Head);
        }
    }

    collapse_preview_text(&arguments.to_string(), 4, 96, PreviewCollapse::Head)
}

pub(crate) fn tool_output_detail_lines_from_preview(
    tool_name: &str,
    output_preview: &str,
    structured_output_preview: Option<&str>,
) -> Vec<String> {
    let structured =
        structured_output_preview.and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    tool_output_detail_lines(tool_name, output_preview, structured.as_ref())
}

pub(crate) fn tool_output_detail_lines(
    tool_name: &str,
    output_preview: &str,
    structured: Option<&Value>,
) -> Vec<String> {
    if tool_name == "bash" {
        return bash_output_detail_lines(output_preview, structured);
    }

    if let Some(detail_lines) =
        file_mutation_output_detail_lines(tool_name, output_preview, structured)
    {
        return detail_lines;
    }

    generic_output_detail_lines(output_preview)
}

fn bash_output_detail_lines(output_preview: &str, structured: Option<&Value>) -> Vec<String> {
    let mut detail_lines = Vec::new();

    let exit_code = structured
        .and_then(|value| value.get("exit_code"))
        .and_then(Value::as_i64);
    if let Some(exit_code) = exit_code {
        detail_lines.push(format!("  └ exit {exit_code}"));
    }

    let timed_out = structured
        .and_then(|value| value.get("timed_out"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if timed_out {
        detail_lines.push("  └ timed out".to_string());
    }

    let stdout = structured
        .and_then(|value| value.pointer("/stdout/text"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let stderr = structured
        .and_then(|value| value.pointer("/stderr/text"))
        .and_then(Value::as_str)
        .unwrap_or_default();

    let stdout_preview =
        collapse_command_output(stdout, exit_code, timed_out, !stderr.trim().is_empty());
    let stderr_preview = if stderr.trim().is_empty() {
        Vec::new()
    } else {
        collapse_preview_text(stderr.trim_end(), 8, 120, PreviewCollapse::Tail)
    };

    match (stdout_preview.is_empty(), stderr_preview.is_empty()) {
        (false, false) => {
            detail_lines.extend(named_detail_block("stdout", &stdout_preview));
            detail_lines.extend(named_detail_block("stderr", &stderr_preview));
        }
        (false, true) => {
            detail_lines.extend(prefixed_detail_lines(&stdout_preview));
        }
        (true, false) => {
            detail_lines.extend(named_detail_block("stderr", &stderr_preview));
        }
        (true, true) => {
            detail_lines.extend(generic_output_detail_lines(output_preview));
        }
    }

    detail_lines
}

fn collapse_command_output(
    output: &str,
    exit_code: Option<i64>,
    timed_out: bool,
    has_stderr: bool,
) -> Vec<String> {
    let trimmed = output.trim_end();
    if trimmed.is_empty() {
        return Vec::new();
    }

    collapse_preview_text(
        trimmed,
        12,
        120,
        command_output_collapse(exit_code, timed_out, has_stderr),
    )
}

fn generic_output_detail_lines(output_preview: &str) -> Vec<String> {
    let trimmed = output_preview.trim();
    if trimmed.is_empty() || trimmed == "<empty>" {
        return Vec::new();
    }

    if output_preview.lines().count() > 1 || output_preview.chars().count() > 96 {
        return prefixed_detail_lines(&collapse_preview_text(
            output_preview,
            8,
            120,
            PreviewCollapse::HeadTail,
        ));
    }

    vec![format!("  └ {}", inline_preview_text(output_preview, 96))]
}

fn file_mutation_output_detail_lines(
    tool_name: &str,
    output_preview: &str,
    structured: Option<&Value>,
) -> Option<Vec<String>> {
    if !matches!(tool_name, "write" | "edit" | "patch") {
        return None;
    }

    let mut detail_lines = Vec::new();
    if let Some(summary) = structured
        .and_then(|value| value.get("summary"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
    {
        detail_lines.push(format!("  └ {}", inline_preview_text(summary, 96)));
    } else if let Some(first_line) = output_preview.lines().next().map(str::trim)
        && !first_line.is_empty()
    {
        detail_lines.push(format!("  └ {}", inline_preview_text(first_line, 96)));
    }

    if let Some(before) = structured
        .and_then(|value| value.get("snapshot_before"))
        .and_then(Value::as_str)
    {
        let after = structured
            .and_then(|value| value.get("snapshot_after"))
            .and_then(Value::as_str)
            .unwrap_or("missing");
        detail_lines.push(format!(
            "  └ snapshot {} -> {}",
            inline_preview_text(before, 16),
            inline_preview_text(after, 16)
        ));
    }

    let file_diffs = structured
        .and_then(|value| value.get("file_diffs"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for diff in &file_diffs {
        if let Some(preview) = diff.get("preview").and_then(Value::as_str) {
            let label = diff
                .get("path")
                .and_then(Value::as_str)
                .map(|path| format!("diff {}", inline_preview_text(path, 96)))
                .unwrap_or_else(|| "diff".to_string());
            detail_lines.extend(named_detail_block(
                &label,
                &collapse_preview_text(preview, 16, 120, PreviewCollapse::HeadTail),
            ));
        }
    }

    Some(detail_lines)
}

fn named_detail_block(label: &str, lines: &[String]) -> Vec<String> {
    let mut rendered = vec![format!("  └ {label}")];
    for line in lines.iter().filter(|line| !line.trim().is_empty()) {
        rendered.push(format!("    {line}"));
    }
    rendered
}

fn inline_preview_text(value: &str, max_chars: usize) -> String {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return "<empty>".to_string();
    }

    if collapsed.chars().count() <= max_chars {
        collapsed
    } else {
        format!(
            "{}...",
            collapsed
                .chars()
                .take(max_chars.saturating_sub(3))
                .collect::<String>()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        summarize_tool_entry, tool_arguments_preview_lines, tool_output_detail_lines,
        tool_output_detail_lines_from_preview,
    };
    use serde_json::json;

    #[test]
    fn bash_arguments_render_as_command_preview() {
        let rendered = tool_arguments_preview_lines("bash", &json!({"command": "cargo test"}));

        assert_eq!(rendered, vec!["$ cargo test"]);
    }

    #[test]
    fn bash_output_uses_tree_details_instead_of_fences() {
        let rendered = summarize_tool_entry(
            "• Finished bash",
            tool_output_detail_lines_from_preview(
                "bash",
                "ok",
                Some(
                    &json!({
                        "exit_code": 0,
                        "timed_out": false,
                        "stdout": {"text": "ok"},
                        "stderr": {"text": ""}
                    })
                    .to_string(),
                ),
            ),
        );

        assert!(rendered.contains("  └ exit 0"));
        assert!(rendered.contains("  └ ok"));
        assert!(!rendered.contains("```"));
    }

    #[test]
    fn file_mutations_render_diff_blocks_as_indented_lines() {
        let rendered = tool_output_detail_lines(
            "write",
            "Wrote 18 bytes to src/lib.rs",
            Some(&json!({
                "summary": "Wrote 18 bytes to src/lib.rs",
                "file_diffs": [{
                    "path": "src/lib.rs",
                    "preview": "--- src/lib.rs\n+++ src/lib.rs\n@@ -1,1 +1,1 @@\n-old()\n+new()"
                }]
            })),
        );

        assert!(rendered.iter().any(|line| line == "  └ diff src/lib.rs"));
        assert!(rendered.iter().any(|line| line == "    +new()"));
    }
}
