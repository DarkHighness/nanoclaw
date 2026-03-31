use crate::preview::{PreviewCollapse, collapse_preview_text, command_output_collapse};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolDetailBlockKind {
    Stdout,
    Stderr,
    Diff,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ToolDetail {
    Command(String),
    Meta(String),
    TextBlock(Vec<String>),
    NamedBlock {
        label: String,
        kind: ToolDetailBlockKind,
        lines: Vec<String>,
    },
}

pub(crate) fn serialize_tool_details(details: &[ToolDetail]) -> Vec<String> {
    details
        .iter()
        .flat_map(ToolDetail::serialized_lines)
        .collect()
}

impl ToolDetail {
    pub(crate) fn serialized_lines(&self) -> Vec<String> {
        match self {
            Self::Command(command) | Self::Meta(command) => vec![format!("  └ {command}")],
            Self::TextBlock(lines) => serialize_detail_block(lines),
            Self::NamedBlock { label, lines, .. } => {
                let mut rendered = vec![format!("  └ {label}")];
                rendered.extend(
                    lines
                        .iter()
                        .filter(|line| !line.trim().is_empty())
                        .map(|line| format!("    {line}")),
                );
                rendered
            }
        }
    }
}

pub(crate) fn preview_tool_details(details: &[ToolDetail], max_lines: usize) -> Vec<ToolDetail> {
    let mut remaining = max_lines;
    let mut preview = Vec::new();

    for detail in details {
        let visible_lines = detail.serialized_lines();
        if visible_lines.is_empty() {
            continue;
        }
        if remaining == 0 {
            break;
        }
        if visible_lines.len() <= remaining {
            preview.push(detail.clone());
            remaining -= visible_lines.len();
            continue;
        }

        preview.push(match detail {
            ToolDetail::Command(command) => ToolDetail::Command(command.clone()),
            ToolDetail::Meta(text) => ToolDetail::Meta(text.clone()),
            ToolDetail::TextBlock(lines) => {
                ToolDetail::TextBlock(lines.iter().take(remaining).cloned().collect())
            }
            ToolDetail::NamedBlock { label, kind, lines } => ToolDetail::NamedBlock {
                label: label.clone(),
                kind: *kind,
                lines: lines
                    .iter()
                    .take(remaining.saturating_sub(1))
                    .cloned()
                    .collect(),
            },
        });
        break;
    }

    preview
}

#[cfg(test)]
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

pub(crate) fn tool_arguments_preview_lines(tool_name: &str, arguments: &Value) -> Vec<String> {
    if tool_name == "exec_command" {
        let command = arguments.get("cmd").and_then(Value::as_str);
        if let Some(command) = command.map(str::trim).filter(|command| !command.is_empty()) {
            return collapse_preview_text(&format!("$ {command}"), 4, 96, PreviewCollapse::Head);
        }
    }

    if tool_name == "write_stdin" {
        let session_id = arguments
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("<unknown>");
        let close_stdin = arguments
            .get("close_stdin")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let chars = arguments
            .get("chars")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if close_stdin && chars.is_empty() {
            return vec![format!("close stdin {session_id}")];
        }
        if chars.is_empty() {
            return vec![format!("poll session {session_id}")];
        }
        let mut lines = vec![format!("session {session_id}")];
        lines.extend(collapse_preview_text(
            &format!("stdin {}", chars.escape_default()),
            3,
            96,
            PreviewCollapse::Head,
        ));
        return lines;
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

    if tool_name == "send_input" {
        let target = arguments
            .get("target")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("<unknown>");
        let interrupt = arguments
            .get("interrupt")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut lines = vec![if interrupt {
            format!("interrupt {target}")
        } else {
            format!("message {target}")
        }];
        if let Some(message) = arguments
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            lines.extend(collapse_preview_text(message, 3, 96, PreviewCollapse::Head));
            return lines;
        }
        let item_count = arguments
            .get("items")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        if item_count > 0 {
            lines.push(format!("{item_count} input item(s)"));
        }
        return lines;
    }

    if tool_name == "wait_agent" {
        let target_count = arguments
            .get("targets")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let timeout_ms = arguments.get("timeout_ms").and_then(Value::as_u64);
        return vec![match timeout_ms {
            Some(timeout_ms) => format!("wait {target_count} agent(s) timeout={timeout_ms}ms"),
            None => format!("wait {target_count} agent(s)"),
        }];
    }

    if matches!(tool_name, "resume_agent" | "close_agent") {
        for key in ["id", "target"] {
            if let Some(value) = arguments.get(key).and_then(Value::as_str)
                && !value.trim().is_empty()
            {
                return vec![format!("agent {}", value.trim())];
            }
        }
    }

    for key in ["path", "uri", "query", "prompt", "message", "cmd"] {
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
    serialize_tool_details(&tool_output_details_from_preview(
        tool_name,
        output_preview,
        structured_output_preview,
    ))
}

pub(crate) fn tool_output_details_from_preview(
    tool_name: &str,
    output_preview: &str,
    structured_output_preview: Option<&str>,
) -> Vec<ToolDetail> {
    let structured =
        structured_output_preview.and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    tool_output_details(tool_name, output_preview, structured.as_ref())
}

pub(crate) fn tool_output_detail_lines(
    tool_name: &str,
    output_preview: &str,
    structured: Option<&Value>,
) -> Vec<String> {
    serialize_tool_details(&tool_output_details(tool_name, output_preview, structured))
}

pub(crate) fn tool_output_details(
    tool_name: &str,
    output_preview: &str,
    structured: Option<&Value>,
) -> Vec<ToolDetail> {
    if matches!(tool_name, "exec_command" | "write_stdin") {
        return process_output_details(tool_name, output_preview, structured);
    }

    if let Some(detail_lines) = file_mutation_output_details(tool_name, output_preview, structured)
    {
        return detail_lines;
    }

    generic_output_details(output_preview)
}

pub(crate) fn tool_argument_details(preview_lines: &[String]) -> Vec<ToolDetail> {
    let lines = preview_lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return Vec::new();
    }
    if lines.len() == 1 && lines[0].starts_with("$ ") {
        return vec![ToolDetail::Command(lines[0].clone())];
    }
    vec![ToolDetail::TextBlock(lines)]
}

fn process_output_details(
    tool_name: &str,
    output_preview: &str,
    structured: Option<&Value>,
) -> Vec<ToolDetail> {
    let mut detail_lines = Vec::new();

    if let Some(session_id) = structured
        .and_then(|value| value.get("session_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        detail_lines.push(ToolDetail::Meta(format!("session {session_id}")));
    }

    if tool_name == "write_stdin" {
        if let Some(wrote_chars) = structured
            .and_then(|value| value.get("wrote_chars"))
            .and_then(Value::as_u64)
            .filter(|wrote_chars| *wrote_chars > 0)
        {
            detail_lines.push(ToolDetail::Meta(format!("sent {wrote_chars} char(s)")));
        }
        if structured
            .and_then(|value| value.get("closed_stdin"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            detail_lines.push(ToolDetail::Meta("closed stdin".to_string()));
        }
    }

    if let Some(state) = structured
        .and_then(|value| value.get("state"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "completed")
    {
        detail_lines.push(ToolDetail::Meta(state.to_string()));
    }

    let exit_code = structured
        .and_then(|value| value.get("exit_code"))
        .and_then(Value::as_i64);
    if let Some(exit_code) = exit_code {
        detail_lines.push(ToolDetail::Meta(format!("exit {exit_code}")));
    }

    let timed_out = structured
        .and_then(|value| value.get("timed_out"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if timed_out {
        detail_lines.push(ToolDetail::Meta("timed out".to_string()));
    }
    if let Some(error) = structured
        .and_then(|value| value.get("error"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        detail_lines.push(ToolDetail::Meta(format!(
            "error {}",
            inline_preview_text(error, 96)
        )));
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
            detail_lines.push(ToolDetail::NamedBlock {
                label: "stdout".to_string(),
                kind: ToolDetailBlockKind::Stdout,
                lines: stdout_preview,
            });
            detail_lines.push(ToolDetail::NamedBlock {
                label: "stderr".to_string(),
                kind: ToolDetailBlockKind::Stderr,
                lines: stderr_preview,
            });
        }
        (false, true) => {
            detail_lines.push(ToolDetail::TextBlock(stdout_preview));
        }
        (true, false) => {
            detail_lines.push(ToolDetail::NamedBlock {
                label: "stderr".to_string(),
                kind: ToolDetailBlockKind::Stderr,
                lines: stderr_preview,
            });
        }
        (true, true) => {
            if detail_lines.is_empty() {
                detail_lines.extend(generic_output_details(output_preview));
            }
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

fn generic_output_details(output_preview: &str) -> Vec<ToolDetail> {
    let trimmed = output_preview.trim();
    if trimmed.is_empty() || trimmed == "<empty>" {
        return Vec::new();
    }

    if output_preview.lines().count() > 1 || output_preview.chars().count() > 96 {
        return vec![ToolDetail::TextBlock(collapse_preview_text(
            output_preview,
            8,
            120,
            PreviewCollapse::HeadTail,
        ))];
    }

    vec![ToolDetail::Meta(inline_preview_text(output_preview, 96))]
}

fn file_mutation_output_details(
    tool_name: &str,
    output_preview: &str,
    structured: Option<&Value>,
) -> Option<Vec<ToolDetail>> {
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
        detail_lines.push(ToolDetail::Meta(inline_preview_text(summary, 96)));
    } else if let Some(first_line) = output_preview.lines().next().map(str::trim)
        && !first_line.is_empty()
    {
        detail_lines.push(ToolDetail::Meta(inline_preview_text(first_line, 96)));
    }

    if let Some(before) = structured
        .and_then(|value| value.get("snapshot_before"))
        .and_then(Value::as_str)
    {
        let after = structured
            .and_then(|value| value.get("snapshot_after"))
            .and_then(Value::as_str)
            .unwrap_or("missing");
        detail_lines.push(ToolDetail::Meta(format!(
            "snapshot {} -> {}",
            inline_preview_text(before, 16),
            inline_preview_text(after, 16)
        )));
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
            detail_lines.push(ToolDetail::NamedBlock {
                label,
                kind: ToolDetailBlockKind::Diff,
                lines: collapse_preview_text(preview, 16, 120, PreviewCollapse::HeadTail),
            });
        }
    }

    Some(detail_lines)
}

fn serialize_detail_block(lines: &[String]) -> Vec<String> {
    let mut rendered = Vec::new();
    for (index, line) in lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
    {
        if index == 0 {
            rendered.push(format!("  └ {line}"));
        } else {
            rendered.push(format!("    {line}"));
        }
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
    fn exec_command_arguments_render_as_command_preview() {
        let rendered = tool_arguments_preview_lines("exec_command", &json!({"cmd": "cargo test"}));

        assert_eq!(rendered, vec!["$ cargo test"]);
    }

    #[test]
    fn write_stdin_arguments_render_as_session_and_input_preview() {
        let rendered = tool_arguments_preview_lines(
            "write_stdin",
            &json!({"session_id": "exec_123", "chars": "hello\\n"}),
        );

        assert_eq!(rendered[0], "session exec_123");
        assert!(rendered[1].contains("stdin hello\\\\n"));
    }

    #[test]
    fn send_input_arguments_render_target_and_message_preview() {
        let rendered = tool_arguments_preview_lines(
            "send_input",
            &json!({"target": "agent_123", "message": "focus the failing test"}),
        );

        assert_eq!(rendered[0], "message agent_123");
        assert!(rendered[1].contains("focus the failing test"));
    }

    #[test]
    fn wait_and_close_arguments_render_codex_style_target_fields() {
        let wait = tool_arguments_preview_lines(
            "wait_agent",
            &json!({"targets": ["agent_1", "agent_2"], "timeout_ms": 5000}),
        );
        let close = tool_arguments_preview_lines("close_agent", &json!({"target": "agent_1"}));
        let resume = tool_arguments_preview_lines("resume_agent", &json!({"id": "agent_2"}));

        assert_eq!(wait, vec!["wait 2 agent(s) timeout=5000ms"]);
        assert_eq!(close, vec!["agent agent_1"]);
        assert_eq!(resume, vec!["agent agent_2"]);
    }

    #[test]
    fn exec_command_output_uses_tree_details_instead_of_fences() {
        let rendered = summarize_tool_entry(
            "• Finished exec_command",
            tool_output_detail_lines_from_preview(
                "exec_command",
                "ok",
                Some(
                    &json!({
                        "session_id": "exec-123",
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

    #[test]
    fn exec_command_output_shows_session_and_stdout_details() {
        let rendered = tool_output_detail_lines(
            "exec_command",
            "ok",
            Some(&json!({
                "session_id": "exec_123",
                "state": "completed",
                "exit_code": 0,
                "stdout": {"text": "ok"},
                "stderr": {"text": ""}
            })),
        );

        assert!(rendered.iter().any(|line| line == "  └ session exec_123"));
        assert!(rendered.iter().any(|line| line == "  └ exit 0"));
        assert!(rendered.iter().any(|line| line == "  └ ok"));
    }
}
