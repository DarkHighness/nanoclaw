use crate::preview::{PreviewCollapse, collapse_preview_text, command_output_collapse};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolDetailBlockKind {
    Stdout,
    Stderr,
    Diff,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolRenderKind {
    ExecCommand,
    WriteStdin,
    UpdatePlan,
    UpdateExecution,
    SendInput,
    SpawnAgent,
    WaitAgent,
    ResumeAgent,
    CloseAgent,
    FileMutation,
    Generic,
}

impl ToolRenderKind {
    pub fn classify(tool_name: &str) -> Self {
        match tool_name {
            "exec_command" => Self::ExecCommand,
            "write_stdin" => Self::WriteStdin,
            "update_plan" => Self::UpdatePlan,
            "update_execution" => Self::UpdateExecution,
            "send_input" => Self::SendInput,
            "spawn_agent" => Self::SpawnAgent,
            "wait_agent" => Self::WaitAgent,
            "resume_agent" => Self::ResumeAgent,
            "close_agent" => Self::CloseAgent,
            "write" | "edit" | "patch" => Self::FileMutation,
            _ => Self::Generic,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolDetailLabel {
    Intent,
    Context,
    Session,
    State,
    Result,
    Effect,
    Snapshot,
    Files,
    Output,
    Origin,
    Reason,
    Note,
}

impl ToolDetailLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Intent => "intent",
            Self::Context => "context",
            Self::Session => "session",
            Self::State => "state",
            Self::Result => "result",
            Self::Effect => "effect",
            Self::Snapshot => "snapshot",
            Self::Files => "files",
            Self::Output => "output",
            Self::Origin => "origin",
            Self::Reason => "reason",
            Self::Note => "note",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolCompletionState {
    #[default]
    Neutral,
    Success,
    Failure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolCommandIntent {
    Execute,
    Explore,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolCommandSummary {
    List {
        targets: Vec<String>,
    },
    Read {
        files: Vec<String>,
    },
    Search {
        query: String,
        scope: Option<String>,
    },
    Inspect {
        subject: String,
    },
}

impl ToolCommandSummary {
    pub fn line(&self) -> String {
        match self {
            Self::List { targets } => join_command_subject("List", targets),
            Self::Read { files } => join_command_subject("Read", files),
            Self::Search { query, scope } => {
                if let Some(scope) = scope.as_deref().filter(|scope| !scope.trim().is_empty()) {
                    format!("Search {query} in {scope}")
                } else {
                    format!("Search {query}")
                }
            }
            Self::Inspect { subject } => {
                let subject = subject.trim();
                if subject.is_empty() {
                    "Inspect".to_string()
                } else {
                    format!("Inspect {subject}")
                }
            }
        }
    }

    pub fn merge_from(&mut self, other: &Self) -> bool {
        match (self, other) {
            (Self::Read { files }, Self::Read { files: other_files }) => {
                push_unique_subjects(files, other_files);
                true
            }
            _ => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCommand {
    pub raw: String,
    pub intent: ToolCommandIntent,
    pub summaries: Vec<ToolCommandSummary>,
}

impl ToolCommand {
    pub fn from_preview(preview_line: &str) -> Self {
        let raw = preview_line
            .trim()
            .strip_prefix("$ ")
            .unwrap_or(preview_line.trim())
            .trim()
            .to_string();
        classify_tool_command(&raw)
    }

    pub fn preview_line(&self) -> String {
        format!("$ {}", self.raw)
    }

    pub fn summary_lines(&self) -> Vec<String> {
        self.summaries
            .iter()
            .map(ToolCommandSummary::line)
            .collect()
    }

    pub fn preview_with_summary_lines(&self, max_lines: usize) -> Self {
        if self.intent != ToolCommandIntent::Explore
            || self.summaries.is_empty()
            || self.summaries.len() <= max_lines
        {
            return self.clone();
        }

        Self {
            raw: self.raw.clone(),
            intent: self.intent,
            summaries: self.summaries.iter().take(max_lines).cloned().collect(),
        }
    }

    pub fn merge_exploration(&mut self, other: &Self) -> bool {
        if self.intent != ToolCommandIntent::Explore || other.intent != ToolCommandIntent::Explore {
            return false;
        }

        append_command_summaries(&mut self.summaries, other.summaries.clone());
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolReviewFile {
    pub path: String,
    pub preview_lines: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolReview {
    pub summary: Option<String>,
    pub files: Vec<ToolReviewFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolDetail {
    Command(ToolCommand),
    Meta(String),
    LabeledValue {
        label: ToolDetailLabel,
        value: String,
    },
    LabeledBlock {
        label: ToolDetailLabel,
        lines: Vec<String>,
    },
    ActionHint {
        key_hint: String,
        label: String,
        detail: Option<String>,
    },
    TextBlock(Vec<String>),
    NamedBlock {
        label: String,
        kind: ToolDetailBlockKind,
        lines: Vec<String>,
    },
}

pub fn serialize_tool_details(details: &[ToolDetail]) -> Vec<String> {
    details
        .iter()
        .flat_map(ToolDetail::serialized_lines)
        .collect()
}

impl ToolDetail {
    pub fn serialized_lines(&self) -> Vec<String> {
        match self {
            Self::Command(command) => {
                if command.intent == ToolCommandIntent::Explore {
                    let summary_lines = command.summary_lines();
                    if let Some((first, rest)) = summary_lines.split_first() {
                        let mut rendered = vec![format!("  └ {first}")];
                        rendered.extend(rest.iter().map(|line| format!("    {line}")));
                        return rendered;
                    }
                }

                vec![format!("  └ {}", command.preview_line())]
            }
            Self::Meta(command) => vec![format!("  └ {command}")],
            Self::LabeledValue { label, value } => {
                vec![format!("  └ {} {value}", label.as_str())]
            }
            Self::LabeledBlock { label, lines } => {
                if let Some((first, rest)) = lines.split_first() {
                    let mut rendered = vec![format!("  └ {} {first}", label.as_str())];
                    rendered.extend(
                        rest.iter()
                            .filter(|line| !line.trim().is_empty())
                            .map(|line| format!("    {line}")),
                    );
                    rendered
                } else {
                    vec![format!("  └ {}", label.as_str())]
                }
            }
            Self::ActionHint {
                key_hint,
                label,
                detail,
            } => {
                let mut line = format!("  └ action [{key_hint}] {label}");
                if let Some(detail) = detail.as_deref().filter(|detail| !detail.trim().is_empty()) {
                    line.push_str(" · ");
                    line.push_str(detail);
                }
                vec![line]
            }
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

pub fn preview_tool_details(details: &[ToolDetail], max_lines: usize) -> Vec<ToolDetail> {
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
            ToolDetail::Command(command) => {
                ToolDetail::Command(command.preview_with_summary_lines(remaining.max(1)))
            }
            ToolDetail::Meta(text) => ToolDetail::Meta(text.clone()),
            ToolDetail::LabeledValue { label, value } => ToolDetail::LabeledValue {
                label: label.clone(),
                value: value.clone(),
            },
            ToolDetail::LabeledBlock { label, lines } => ToolDetail::LabeledBlock {
                label: label.clone(),
                lines: lines.iter().take(remaining.max(1)).cloned().collect(),
            },
            ToolDetail::ActionHint {
                key_hint,
                label,
                detail,
            } => ToolDetail::ActionHint {
                key_hint: key_hint.clone(),
                label: label.clone(),
                detail: detail.clone(),
            },
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
pub fn summarize_tool_entry(headline: impl Into<String>, detail_lines: Vec<String>) -> String {
    let mut lines = vec![headline.into()];
    lines.extend(
        detail_lines
            .into_iter()
            .filter(|line| !line.trim().is_empty()),
    );
    lines.join("\n")
}

pub fn tool_arguments_preview_lines(tool_name: &str, arguments: &Value) -> Vec<String> {
    match ToolRenderKind::classify(tool_name) {
        ToolRenderKind::ExecCommand => {
            let command = arguments.get("cmd").and_then(Value::as_str);
            if let Some(command) = command.map(str::trim).filter(|command| !command.is_empty()) {
                return collapse_preview_text(
                    &format!("$ {command}"),
                    4,
                    96,
                    PreviewCollapse::Head,
                );
            }
        }
        ToolRenderKind::WriteStdin => {
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
        ToolRenderKind::UpdatePlan => {
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
        ToolRenderKind::SendInput => {
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
                format!("interrupt+restart {target}")
            } else {
                format!("queue input {target}")
            }];
            if let Some(message) = arguments
                .get("message")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                lines.extend(collapse_preview_text(message, 3, 96, PreviewCollapse::Head));
            }
            lines.extend(preview_input_item_argument_lines(arguments.get("items"), 2));
            return lines;
        }
        ToolRenderKind::SpawnAgent => {
            let agent_type = arguments
                .get("agent_type")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("general-purpose");
            let model = arguments
                .get("model")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let reasoning_effort = arguments
                .get("reasoning_effort")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let fork_context = arguments
                .get("fork_context")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let mut summary = format!("spawn {agent_type}");
            if fork_context {
                summary.push_str(" forked");
            }
            if let Some(model) = model {
                summary.push_str(&format!(" model={model}"));
            }
            if let Some(reasoning_effort) = reasoning_effort {
                summary.push_str(&format!(" effort={reasoning_effort}"));
            }
            let mut lines = vec![summary];
            if let Some(message) = arguments
                .get("message")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                lines.extend(collapse_preview_text(message, 3, 96, PreviewCollapse::Head));
            }
            lines.extend(preview_input_item_argument_lines(arguments.get("items"), 2));
            return lines;
        }
        ToolRenderKind::WaitAgent => {
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
        ToolRenderKind::ResumeAgent | ToolRenderKind::CloseAgent => {
            for key in ["id", "target"] {
                if let Some(value) = arguments.get(key).and_then(Value::as_str)
                    && !value.trim().is_empty()
                {
                    return vec![format!("agent {}", value.trim())];
                }
            }
        }
        ToolRenderKind::UpdateExecution
        | ToolRenderKind::FileMutation
        | ToolRenderKind::Generic => {}
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

fn preview_input_item_argument_lines(items: Option<&Value>, max_items: usize) -> Vec<String> {
    let Some(items) = items.and_then(Value::as_array) else {
        return Vec::new();
    };
    if items.is_empty() {
        return Vec::new();
    }

    let mut lines = items
        .iter()
        .take(max_items)
        .filter_map(render_input_item_argument_summary)
        .collect::<Vec<_>>();
    let remaining = items.len().saturating_sub(max_items);
    if remaining > 0 {
        lines.push(format!("+{remaining} more item(s)"));
    }
    lines
}

fn render_input_item_argument_summary(item: &Value) -> Option<String> {
    let item_type = item
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("item");
    if item_type == "text" {
        return item
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| truncate_inline(value, 72));
    }

    let mut fields = Vec::new();
    for key in ["name", "path", "image_url", "text"] {
        if let Some(value) = item.get(key).and_then(Value::as_str).map(str::trim)
            && !value.is_empty()
        {
            let value = if key == "text" {
                value.replace('\n', " ")
            } else {
                value.to_string()
            };
            fields.push(format!("{key}={}", truncate_inline(&value, 64)));
        }
    }
    if fields.is_empty() {
        None
    } else {
        Some(format!("[{item_type}] {}", fields.join(" ")))
    }
}

fn truncate_inline(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let mut truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        truncated.push_str("...");
    }
    truncated
}

fn classify_tool_command(raw: &str) -> ToolCommand {
    let tokens = shlex::split(raw).unwrap_or_default();
    let summaries = classify_exploration_summaries(&tokens);
    let intent = if summaries.is_empty() {
        ToolCommandIntent::Execute
    } else {
        ToolCommandIntent::Explore
    };
    ToolCommand {
        raw: raw.to_string(),
        intent,
        summaries,
    }
}

fn classify_exploration_summaries(tokens: &[String]) -> Vec<ToolCommandSummary> {
    if tokens.is_empty() {
        return Vec::new();
    }

    if tokens
        .iter()
        .any(|token| token == "|" || is_shell_redirection_token(token))
    {
        return Vec::new();
    }

    let segments = shell_segments(tokens);
    if segments.is_empty() || !segments.iter().all(|segment| is_read_only_segment(segment)) {
        return Vec::new();
    }

    let mut summaries = Vec::new();
    for segment in segments {
        if let Some(summary) = classify_exploration_segment(segment) {
            append_command_summary(&mut summaries, summary);
        }
    }
    summaries
}

fn shell_segments(tokens: &[String]) -> Vec<&[String]> {
    let mut segments = Vec::new();
    let mut start = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        if matches!(token.as_str(), "|" | "||" | "&&" | ";") {
            if start < index {
                segments.push(&tokens[start..index]);
            }
            start = index + 1;
        }
    }
    if start < tokens.len() {
        segments.push(&tokens[start..]);
    }
    segments
}

fn is_read_only_segment(segment: &[String]) -> bool {
    let Some(command) = segment.first().map(String::as_str) else {
        return false;
    };
    match command {
        "ls" | "tree" | "find" | "rg" | "grep" | "cat" | "head" | "tail" | "nl" | "pwd"
        | "stat" | "wc" => true,
        "sed" => !segment
            .iter()
            .skip(1)
            .any(|token| token == "-i" || token.starts_with("-i")),
        "git" => matches!(
            segment.get(1).map(String::as_str),
            Some("status" | "diff" | "log" | "show" | "grep" | "ls-files" | "branch" | "rev-parse")
        ),
        _ => false,
    }
}

fn classify_exploration_segment(segment: &[String]) -> Option<ToolCommandSummary> {
    let command = segment.first()?.as_str();
    match command {
        "ls" | "tree" | "find" => Some(ToolCommandSummary::List {
            targets: summarize_listing_targets(segment),
        }),
        "pwd" => Some(ToolCommandSummary::Inspect {
            subject: "working directory".to_string(),
        }),
        "rg" | "grep" => summarize_search(segment),
        "cat" | "head" | "tail" | "nl" => summarize_read(segment, 1),
        "sed" => summarize_sed_read(segment),
        "stat" | "wc" => summarize_inspect_paths(segment),
        "git" => summarize_git_inspection(segment),
        _ => None,
    }
}

fn summarize_listing_targets(segment: &[String]) -> Vec<String> {
    let mut targets = positional_args(segment, 1)
        .into_iter()
        .filter(|token| !is_shell_redirection_token(token))
        .map(|value| compact_command_target(&value))
        .collect::<Vec<_>>();
    if targets.is_empty() {
        targets.push(".".to_string());
    }
    targets
}

fn summarize_search(segment: &[String]) -> Option<ToolCommandSummary> {
    let positionals = positional_args(segment, 1);
    let pattern = positionals
        .first()
        .map(|value| compact_inline_summary(value))?;
    let scope = positionals
        .get(1)
        .map(|scope| compact_command_target(scope))
        .filter(|scope| !scope.is_empty());
    Some(ToolCommandSummary::Search {
        query: pattern,
        scope,
    })
}

fn summarize_read(segment: &[String], start_index: usize) -> Option<ToolCommandSummary> {
    let files = positional_args(segment, start_index)
        .into_iter()
        .filter(|token| !is_shell_redirection_token(token))
        .map(|value| compact_command_target(&value))
        .collect::<Vec<_>>();
    if files.is_empty() {
        return None;
    }
    Some(ToolCommandSummary::Read { files })
}

fn summarize_sed_read(segment: &[String]) -> Option<ToolCommandSummary> {
    let mut script_consumed = false;
    let mut files = Vec::new();

    for token in segment.iter().skip(1) {
        if token == "--" {
            script_consumed = true;
            continue;
        }
        if token.starts_with('-') && !script_consumed {
            continue;
        }
        if !script_consumed {
            script_consumed = true;
            continue;
        }
        if is_shell_redirection_token(token) {
            continue;
        }
        files.push(compact_command_target(token));
    }

    if files.is_empty() {
        None
    } else {
        Some(ToolCommandSummary::Read { files })
    }
}

fn summarize_inspect_paths(segment: &[String]) -> Option<ToolCommandSummary> {
    let targets = positional_args(segment, 1)
        .into_iter()
        .filter(|token| !is_shell_redirection_token(token))
        .map(|value| compact_command_target(&value))
        .collect::<Vec<_>>();
    let subject = if targets.is_empty() {
        segment.first()?.clone()
    } else {
        compact_subject_list(&targets)
    };
    Some(ToolCommandSummary::Inspect { subject })
}

fn summarize_git_inspection(segment: &[String]) -> Option<ToolCommandSummary> {
    let subcommand = segment.get(1).map(String::as_str)?;
    let mut subject = format!("git {subcommand}");
    let targets = positional_args(segment, 2);
    if let Some(target) = targets.first() {
        subject.push(' ');
        subject.push_str(&compact_command_target(target));
    }
    Some(ToolCommandSummary::Inspect { subject })
}

fn positional_args(segment: &[String], start_index: usize) -> Vec<String> {
    let mut after_double_dash = false;
    let mut args = Vec::new();
    for token in segment.iter().skip(start_index) {
        if token == "--" {
            after_double_dash = true;
            continue;
        }
        if !after_double_dash && token.starts_with('-') {
            continue;
        }
        args.push(token.clone());
    }
    args
}

fn compact_command_target(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return ".".to_string();
    }
    let display = trimmed.trim_end_matches('/');
    display
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or(display)
        .to_string()
}

fn compact_subject_list(values: &[String]) -> String {
    match values {
        [] => String::new(),
        [only] => only.clone(),
        _ => values.join(", "),
    }
}

fn compact_inline_summary(value: &str) -> String {
    truncate_inline(value.trim_matches('\'').trim_matches('"'), 48)
}

fn append_command_summaries(
    target: &mut Vec<ToolCommandSummary>,
    incoming: Vec<ToolCommandSummary>,
) {
    for summary in incoming {
        append_command_summary(target, summary);
    }
}

fn append_command_summary(target: &mut Vec<ToolCommandSummary>, summary: ToolCommandSummary) {
    if let Some(last) = target.last_mut()
        && last.merge_from(&summary)
    {
        return;
    }
    target.push(summary);
}

fn push_unique_subjects(target: &mut Vec<String>, incoming: &[String]) {
    for value in incoming {
        if !target.iter().any(|existing| existing == value) {
            target.push(value.clone());
        }
    }
}

fn join_command_subject(title: &str, values: &[String]) -> String {
    let subject = compact_subject_list(values);
    if subject.is_empty() {
        title.to_string()
    } else {
        format!("{title} {subject}")
    }
}

fn is_shell_redirection_token(token: &str) -> bool {
    let trimmed = token.trim();
    !trimmed.is_empty()
        && [">", ">>", "<", "<<", "1>", "1>>", "2>", "2>>", "&>", "&>>"]
            .iter()
            .any(|prefix| trimmed.starts_with(prefix))
}

pub fn tool_completion_state_from_preview(
    tool_name: &str,
    structured_output_preview: Option<&str>,
) -> ToolCompletionState {
    let structured =
        structured_output_preview.and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    tool_completion_state(tool_name, structured.as_ref())
}

pub fn tool_completion_state(tool_name: &str, structured: Option<&Value>) -> ToolCompletionState {
    let Some(structured) = structured else {
        return ToolCompletionState::Success;
    };

    if structured
        .get("error")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some()
    {
        return ToolCompletionState::Failure;
    }

    if structured
        .get("timed_out")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return ToolCompletionState::Failure;
    }

    if let Some(exit_code) = structured.get("exit_code").and_then(Value::as_i64) {
        return if exit_code == 0 {
            ToolCompletionState::Success
        } else {
            ToolCompletionState::Failure
        };
    }

    if let Some(state) = structured
        .get("state")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return match state {
            "failed" | "error" | "cancelled" => ToolCompletionState::Failure,
            _ => ToolCompletionState::Success,
        };
    }

    match ToolRenderKind::classify(tool_name) {
        ToolRenderKind::ExecCommand | ToolRenderKind::WriteStdin => ToolCompletionState::Neutral,
        ToolRenderKind::UpdatePlan
        | ToolRenderKind::UpdateExecution
        | ToolRenderKind::SendInput
        | ToolRenderKind::SpawnAgent
        | ToolRenderKind::WaitAgent
        | ToolRenderKind::ResumeAgent
        | ToolRenderKind::CloseAgent
        | ToolRenderKind::FileMutation
        | ToolRenderKind::Generic => ToolCompletionState::Success,
    }
}

pub fn tool_output_detail_lines_from_preview(
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

pub fn tool_output_details_from_preview(
    tool_name: &str,
    output_preview: &str,
    structured_output_preview: Option<&str>,
) -> Vec<ToolDetail> {
    let structured =
        structured_output_preview.and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    tool_output_details(tool_name, output_preview, structured.as_ref())
}

pub fn tool_output_detail_lines(
    tool_name: &str,
    output_preview: &str,
    structured: Option<&Value>,
) -> Vec<String> {
    serialize_tool_details(&tool_output_details(tool_name, output_preview, structured))
}

pub fn tool_output_details(
    tool_name: &str,
    output_preview: &str,
    structured: Option<&Value>,
) -> Vec<ToolDetail> {
    match ToolRenderKind::classify(tool_name) {
        ToolRenderKind::ExecCommand | ToolRenderKind::WriteStdin => {
            return process_output_details(tool_name, output_preview, structured);
        }
        ToolRenderKind::FileMutation => {
            if let Some(detail_lines) =
                file_mutation_output_details(tool_name, output_preview, structured)
            {
                return detail_lines;
            }
        }
        ToolRenderKind::UpdatePlan
        | ToolRenderKind::UpdateExecution
        | ToolRenderKind::SendInput
        | ToolRenderKind::SpawnAgent
        | ToolRenderKind::WaitAgent
        | ToolRenderKind::ResumeAgent
        | ToolRenderKind::CloseAgent
        | ToolRenderKind::Generic => {}
    }

    generic_output_details(output_preview)
}

pub fn tool_review_from_preview(
    tool_name: &str,
    structured_output_preview: Option<&str>,
) -> Option<ToolReview> {
    let structured =
        structured_output_preview.and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    tool_review(tool_name, structured.as_ref())
}

pub fn tool_review(tool_name: &str, structured: Option<&Value>) -> Option<ToolReview> {
    if ToolRenderKind::classify(tool_name) != ToolRenderKind::FileMutation {
        return None;
    }

    let structured = structured?;
    let summary = structured
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
        .map(str::to_string);
    let files = structured
        .get("file_diffs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|diff| {
            let preview = diff
                .get("preview")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|preview| !preview.is_empty())?;
            let path = diff
                .get("path")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .unwrap_or("diff")
                .to_string();
            Some(ToolReviewFile {
                path,
                preview_lines: collapse_preview_text(preview, 48, 120, PreviewCollapse::HeadTail),
            })
        })
        .collect::<Vec<_>>();
    if files.is_empty() {
        None
    } else {
        Some(ToolReview { summary, files })
    }
}

pub fn tool_argument_details(preview_lines: &[String]) -> Vec<ToolDetail> {
    let lines = preview_lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return Vec::new();
    }
    if lines.len() == 1 && lines[0].starts_with("$ ") {
        return vec![ToolDetail::Command(ToolCommand::from_preview(&lines[0]))];
    }

    let mut details = vec![ToolDetail::LabeledValue {
        label: ToolDetailLabel::Intent,
        value: lines[0].clone(),
    }];
    if let Some(remaining) = lines.get(1..) {
        if remaining.len() == 1 {
            details.push(ToolDetail::LabeledValue {
                label: ToolDetailLabel::Context,
                value: remaining[0].clone(),
            });
        } else if !remaining.is_empty() {
            details.push(ToolDetail::LabeledBlock {
                label: ToolDetailLabel::Context,
                lines: remaining.to_vec(),
            });
        }
    }
    details
}

pub fn compact_successful_exploration_details(
    detail_lines: &mut Vec<ToolDetail>,
    completion: ToolCompletionState,
) {
    if completion != ToolCompletionState::Success {
        return;
    }

    let is_exploration = detail_lines.iter().any(|detail| {
        matches!(
            detail,
            ToolDetail::Command(ToolCommand {
                intent: ToolCommandIntent::Explore,
                ..
            })
        )
    });
    if !is_exploration {
        return;
    }

    detail_lines.retain(|detail| matches!(detail, ToolDetail::Command(_)));
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
        detail_lines.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Session,
            value: session_id.to_string(),
        });
    }

    if tool_name == "write_stdin" {
        if let Some(wrote_chars) = structured
            .and_then(|value| value.get("wrote_chars"))
            .and_then(Value::as_u64)
            .filter(|wrote_chars| *wrote_chars > 0)
        {
            detail_lines.push(ToolDetail::LabeledValue {
                label: ToolDetailLabel::Effect,
                value: format!("sent {wrote_chars} char(s)"),
            });
        }
        if structured
            .and_then(|value| value.get("closed_stdin"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            detail_lines.push(ToolDetail::LabeledValue {
                label: ToolDetailLabel::Effect,
                value: "closed stdin".to_string(),
            });
        }
    }

    if let Some(state) = structured
        .and_then(|value| value.get("state"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "completed")
    {
        detail_lines.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::State,
            value: state.to_string(),
        });
    }

    let exit_code = structured
        .and_then(|value| value.get("exit_code"))
        .and_then(Value::as_i64);
    if let Some(exit_code) = exit_code {
        detail_lines.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Result,
            value: format!("exit {exit_code}"),
        });
    }

    let timed_out = structured
        .and_then(|value| value.get("timed_out"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if timed_out {
        detail_lines.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Result,
            value: "timed out".to_string(),
        });
    }
    if let Some(error) = structured
        .and_then(|value| value.get("error"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        detail_lines.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Result,
            value: format!("error {}", inline_preview_text(error, 96)),
        });
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
    let has_stdout_preview = !stdout_preview.is_empty();
    let has_stderr_preview = !stderr_preview.is_empty();

    if has_stdout_preview {
        detail_lines.push(ToolDetail::NamedBlock {
            label: "stdout".to_string(),
            kind: ToolDetailBlockKind::Stdout,
            lines: stdout_preview,
        });
    }
    if has_stderr_preview {
        detail_lines.push(ToolDetail::NamedBlock {
            label: "stderr".to_string(),
            kind: ToolDetailBlockKind::Stderr,
            lines: stderr_preview,
        });
    }

    if !has_stdout_preview && !has_stderr_preview && detail_lines.is_empty() {
        detail_lines.extend(generic_output_details(output_preview));
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

    vec![ToolDetail::LabeledValue {
        label: ToolDetailLabel::Output,
        value: inline_preview_text(output_preview, 96),
    }]
}

fn file_mutation_output_details(
    tool_name: &str,
    output_preview: &str,
    structured: Option<&Value>,
) -> Option<Vec<ToolDetail>> {
    if ToolRenderKind::classify(tool_name) != ToolRenderKind::FileMutation {
        return None;
    }

    let mut detail_lines = Vec::new();
    let review = tool_review(tool_name, structured);
    if let Some(summary) = review
        .as_ref()
        .and_then(|review| review.summary.as_deref())
        .filter(|summary| !summary.is_empty())
    {
        detail_lines.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Effect,
            value: inline_preview_text(summary, 96),
        });
    } else if let Some(first_line) = output_preview.lines().next().map(str::trim)
        && !first_line.is_empty()
    {
        detail_lines.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Effect,
            value: inline_preview_text(first_line, 96),
        });
    }

    if let Some(before) = structured
        .and_then(|value| value.get("snapshot_before"))
        .and_then(Value::as_str)
    {
        let after = structured
            .and_then(|value| value.get("snapshot_after"))
            .and_then(Value::as_str)
            .unwrap_or("missing");
        detail_lines.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Snapshot,
            value: format!(
                "{} -> {}",
                inline_preview_text(before, 16),
                inline_preview_text(after, 16)
            ),
        });
    }

    if let Some(review) = review {
        detail_lines.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Files,
            value: review_file_summary(&review.files),
        });
        detail_lines.push(ToolDetail::ActionHint {
            key_hint: "r".to_string(),
            label: if review.files.len() == 1 {
                "review diff".to_string()
            } else {
                "review diffs".to_string()
            },
            detail: Some(if review.files.len() == 1 {
                review.files[0].path.clone()
            } else {
                format!("{} file(s)", review.files.len())
            }),
        });
    }

    Some(detail_lines)
}

fn review_file_summary(files: &[ToolReviewFile]) -> String {
    match files {
        [] => "no files".to_string(),
        [file] => file.path.clone(),
        [first, second] => format!("2 file(s) · {} · {}", first.path, second.path),
        [first, second, rest @ ..] => format!(
            "{} file(s) · {} · {} · +{} more",
            rest.len() + 2,
            first.path,
            second.path,
            rest.len()
        ),
    }
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
        tool_output_detail_lines_from_preview, tool_review,
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

        assert_eq!(rendered[0], "queue input agent_123");
        assert!(rendered[1].contains("focus the failing test"));
    }

    #[test]
    fn send_input_interrupt_arguments_surface_restart_and_item_context() {
        let rendered = tool_arguments_preview_lines(
            "send_input",
            &json!({
                "target": "agent_123",
                "interrupt": true,
                "items": [
                    {"type": "local_image", "path": "/tmp/failure.png", "text": "latest failure screenshot"},
                    {"type": "text", "text": "focus the diff hunk"}
                ]
            }),
        );

        assert_eq!(rendered[0], "interrupt+restart agent_123");
        assert!(rendered[1].contains("[local_image] path=/tmp/failure.png"));
        assert!(rendered[2].contains("focus the diff hunk"));
    }

    #[test]
    fn spawn_agent_arguments_render_role_and_overrides_preview() {
        let rendered = tool_arguments_preview_lines(
            "spawn_agent",
            &json!({
                "agent_type": "reviewer",
                "message": "Inspect the patch.",
                "fork_context": true,
                "model": "gpt-5.4",
                "reasoning_effort": "high"
            }),
        );

        assert_eq!(
            rendered[0],
            "spawn reviewer forked model=gpt-5.4 effort=high"
        );
        assert!(rendered[1].contains("Inspect the patch."));
    }

    #[test]
    fn spawn_agent_arguments_surface_item_summaries_when_present() {
        let rendered = tool_arguments_preview_lines(
            "spawn_agent",
            &json!({
                "agent_type": "reviewer",
                "items": [
                    {"type": "local_image", "path": "artifacts/failure.png"},
                    {"type": "mention", "path": "app://workspace/snapshot", "name": "workspace"}
                ]
            }),
        );

        assert_eq!(rendered[0], "spawn reviewer");
        assert!(rendered[1].contains("[local_image] path=artifacts/failure.png"));
        assert!(rendered[2].contains("[mention] name=workspace path=app://workspace/snapshot"));
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

        assert!(rendered.contains("  └ result exit 0"));
        assert!(rendered.contains("  └ stdout"));
        assert!(rendered.contains("    ok"));
        assert!(!rendered.contains("```"));
    }

    #[test]
    fn file_mutations_surface_files_and_review_action() {
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

        assert!(
            rendered
                .iter()
                .any(|line| line == "  └ effect Wrote 18 bytes to src/lib.rs")
        );
        assert!(rendered.iter().any(|line| line == "  └ files src/lib.rs"));
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("action [r] review diff"))
        );
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
        assert!(rendered.iter().any(|line| line == "  └ result exit 0"));
        assert!(rendered.iter().any(|line| line == "  └ stdout"));
        assert!(rendered.iter().any(|line| line == "    ok"));
    }

    #[test]
    fn file_mutation_review_extracts_structured_diff_preview() {
        let review = tool_review(
            "write",
            Some(&json!({
                "summary": "Wrote 18 bytes to src/lib.rs",
                "file_diffs": [{
                    "path": "src/lib.rs",
                    "preview": "--- src/lib.rs\n+++ src/lib.rs\n@@ -1,1 +1,1 @@\n-old()\n+new()"
                }]
            })),
        )
        .expect("expected review");

        assert_eq!(
            review.summary.as_deref(),
            Some("Wrote 18 bytes to src/lib.rs")
        );
        assert_eq!(review.files.len(), 1);
        assert_eq!(review.files[0].path, "src/lib.rs");
        assert!(
            review.files[0]
                .preview_lines
                .iter()
                .any(|line| line == "+new()")
        );
    }
}
