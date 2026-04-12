use crate::preview::{PreviewCollapse, collapse_preview_text, command_output_collapse};
use serde_json::{Map, Value};

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
    CronCreate,
    CronList,
    CronDelete,
    NotebookEdit,
    NotebookRead,
    CodeSearch,
    CodeDiagnostics,
    BrowserOpen,
    MonitorStart,
    MonitorList,
    MonitorStop,
    WorktreeEnter,
    WorktreeList,
    WorktreeExit,
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
            "cron_create" => Self::CronCreate,
            "cron_list" => Self::CronList,
            "cron_delete" => Self::CronDelete,
            "notebook_edit" => Self::NotebookEdit,
            "notebook_read" => Self::NotebookRead,
            "code_search" => Self::CodeSearch,
            "code_diagnostics" => Self::CodeDiagnostics,
            "browser_open" => Self::BrowserOpen,
            "monitor_start" => Self::MonitorStart,
            "monitor_list" => Self::MonitorList,
            "monitor_stop" => Self::MonitorStop,
            "worktree_enter" => Self::WorktreeEnter,
            "worktree_list" => Self::WorktreeList,
            "worktree_exit" => Self::WorktreeExit,
            "send_input" => Self::SendInput,
            "spawn_agent" => Self::SpawnAgent,
            "wait_agent" => Self::WaitAgent,
            "resume_agent" => Self::ResumeAgent,
            "close_agent" => Self::CloseAgent,
            "write" | "edit" | "patch_files" => Self::FileMutation,
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
            Self::Intent => "Intent",
            Self::Context => "Context",
            Self::Session => "Session",
            Self::State => "State",
            Self::Result => "Result",
            Self::Effect => "Effect",
            Self::Snapshot => "Snapshot",
            Self::Files => "Files",
            Self::Output => "Output",
            Self::Origin => "Origin",
            Self::Reason => "Reason",
            Self::Note => "Note",
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
        ToolRenderKind::CronCreate => {
            let mut lines = vec!["Schedule automation".to_string()];
            if let Some(summary) = arguments
                .get("summary")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                lines.push(truncate_inline(summary, 88));
            } else if let Some(prompt) = arguments
                .get("prompt")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                lines.push(truncate_inline(prompt.lines().next().unwrap_or(prompt), 88));
            }
            if let Some(schedule) = render_cron_schedule_argument(arguments.get("schedule")) {
                lines.push(schedule);
            }
            if let Some(role) = arguments
                .get("role")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                lines.push(format!("role {role}"));
            }
            return lines;
        }
        ToolRenderKind::CronList => {
            return vec!["List automations".to_string()];
        }
        ToolRenderKind::CronDelete => {
            let cron_id = arguments
                .get("cron_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("<unknown>");
            return vec![format!("Cancel automation {cron_id}")];
        }
        ToolRenderKind::NotebookEdit => {
            let path = arguments
                .get("path")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("<unknown>");
            let mut lines = vec![format!("Edit notebook {}", truncate_inline(path, 80))];
            if let Some(operations) = arguments.get("operations").and_then(Value::as_array) {
                lines.extend(render_notebook_edit_operation_preview_lines(operations, 3));
            }
            if arguments
                .get("expected_snapshot")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_some()
            {
                lines.push("snapshot guard enabled".to_string());
            }
            return lines;
        }
        ToolRenderKind::NotebookRead => {
            let path = arguments
                .get("path")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("<unknown>");
            let mut lines = vec![format!("Read notebook {}", truncate_inline(path, 80))];
            let start_cell = arguments.get("start_cell").and_then(Value::as_u64);
            let end_cell = arguments.get("end_cell").and_then(Value::as_u64);
            let cell_count = arguments.get("cell_count").and_then(Value::as_u64);
            match (start_cell, end_cell, cell_count) {
                (Some(start), Some(end), _) => lines.push(format!("cells {start}-{end}")),
                (Some(start), None, Some(count)) => {
                    lines.push(format!("start cell {start}, count {count}"))
                }
                (Some(start), None, None) => lines.push(format!("start cell {start}")),
                (None, None, Some(count)) => lines.push(format!("count {count}")),
                _ => {}
            }
            if arguments
                .get("include_outputs")
                .and_then(Value::as_bool)
                .is_some_and(|include_outputs| !include_outputs)
            {
                lines.push("outputs hidden".to_string());
            }
            return lines;
        }
        ToolRenderKind::CodeSearch => {
            let query = arguments
                .get("query")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("<empty>");
            if let Some(path_prefix) = arguments
                .get("path_prefix")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                return vec![format!(
                    "Search code for {} in {}",
                    truncate_inline(query, 72),
                    truncate_inline(path_prefix, 72)
                )];
            }
            return vec![format!("Search code for {}", truncate_inline(query, 80))];
        }
        ToolRenderKind::CodeDiagnostics => {
            if let Some(path) = arguments
                .get("path")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                return vec![format!(
                    "Inspect diagnostics for {}",
                    truncate_inline(path, 80)
                )];
            }
            return vec!["Inspect workspace diagnostics".to_string()];
        }
        ToolRenderKind::BrowserOpen => {
            let url = arguments
                .get("url")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("<unknown>");
            let mut lines = vec![format!("Open browser {}", truncate_inline(url, 80))];
            if arguments
                .get("headless")
                .and_then(Value::as_bool)
                .is_some_and(|headless| !headless)
            {
                lines.push("mode headful".to_string());
            }
            if let Some(viewport) = arguments.get("viewport").and_then(Value::as_object) {
                let width = viewport.get("width").and_then(Value::as_u64).unwrap_or(0);
                let height = viewport.get("height").and_then(Value::as_u64).unwrap_or(0);
                if width > 0 && height > 0 {
                    lines.push(format!("viewport {width}x{height}"));
                }
            }
            return lines;
        }
        ToolRenderKind::MonitorStart => {
            let command = arguments.get("cmd").and_then(Value::as_str);
            if let Some(command) = command.map(str::trim).filter(|command| !command.is_empty()) {
                let mut lines = vec![format!("Start monitor {}", truncate_inline(command, 80))];
                if let Some(workdir) = arguments
                    .get("workdir")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    lines.push(format!("cwd {}", truncate_inline(workdir, 72)));
                }
                return lines;
            }
        }
        ToolRenderKind::MonitorList => {
            let include_closed = arguments
                .get("include_closed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            return vec![if include_closed {
                "List monitors including closed".to_string()
            } else {
                "List active monitors".to_string()
            }];
        }
        ToolRenderKind::MonitorStop => {
            if let Some(monitor_id) = arguments
                .get("monitor_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                let mut lines = vec![format!("Stop monitor {monitor_id}")];
                if let Some(reason) = arguments
                    .get("reason")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    lines.push(format!("reason {}", truncate_inline(reason, 72)));
                }
                return lines;
            }
        }
        ToolRenderKind::WorktreeEnter => {
            let mut lines = vec!["Enter session worktree".to_string()];
            if let Some(label) = arguments
                .get("label")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                lines.push(format!("label {}", truncate_inline(label, 72)));
            }
            return lines;
        }
        ToolRenderKind::WorktreeList => {
            let include_inactive = arguments
                .get("include_inactive")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            return vec![if include_inactive {
                "List worktrees including inactive".to_string()
            } else {
                "List active worktrees".to_string()
            }];
        }
        ToolRenderKind::WorktreeExit => {
            if let Some(worktree_id) = arguments
                .get("worktree_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                return vec![format!("Exit worktree {worktree_id}")];
            }
            return vec!["Exit current worktree".to_string()];
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
            let dedicated_worktree = arguments
                .get("worktree_mode")
                .and_then(Value::as_str)
                .is_some_and(|value| value.trim() == "dedicated");
            let mut summary = format!("spawn {agent_type}");
            if fork_context {
                summary.push_str(" forked");
            }
            if dedicated_worktree {
                summary.push_str(" worktree=dedicated");
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
        ToolRenderKind::FileMutation | ToolRenderKind::Generic => {}
    }

    if let Some(object) = arguments.as_object() {
        let lines = summarize_generic_argument_object(object);
        if !lines.is_empty() {
            return lines;
        }
    }

    summarize_json_preview_lines(arguments, 4).unwrap_or_else(|| {
        collapse_preview_text(&arguments.to_string(), 4, 96, PreviewCollapse::HeadTail)
    })
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
        ToolRenderKind::ExecCommand | ToolRenderKind::WriteStdin | ToolRenderKind::MonitorStart => {
            ToolCompletionState::Neutral
        }
        ToolRenderKind::CronCreate
        | ToolRenderKind::CronList
        | ToolRenderKind::CronDelete
        | ToolRenderKind::NotebookEdit
        | ToolRenderKind::NotebookRead
        | ToolRenderKind::CodeSearch
        | ToolRenderKind::CodeDiagnostics
        | ToolRenderKind::BrowserOpen
        | ToolRenderKind::MonitorList
        | ToolRenderKind::MonitorStop
        | ToolRenderKind::WorktreeEnter
        | ToolRenderKind::WorktreeList
        | ToolRenderKind::WorktreeExit
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
        ToolRenderKind::CronCreate | ToolRenderKind::CronList | ToolRenderKind::CronDelete => {
            if let Some(details) = cron_output_details(tool_name, structured) {
                return details;
            }
        }
        ToolRenderKind::NotebookEdit => {
            if let Some(details) = notebook_edit_output_details(structured) {
                return details;
            }
        }
        ToolRenderKind::NotebookRead => {
            if let Some(details) = notebook_read_output_details(structured) {
                return details;
            }
        }
        ToolRenderKind::CodeSearch => {
            if let Some(details) = code_search_output_details(structured) {
                return details;
            }
        }
        ToolRenderKind::CodeDiagnostics => {
            if let Some(details) = code_diagnostics_output_details(structured) {
                return details;
            }
        }
        ToolRenderKind::BrowserOpen => {
            if let Some(details) = browser_output_details(structured) {
                return details;
            }
        }
        ToolRenderKind::MonitorStart
        | ToolRenderKind::MonitorList
        | ToolRenderKind::MonitorStop => {
            if let Some(details) = monitor_output_details(tool_name, structured) {
                return details;
            }
        }
        ToolRenderKind::WorktreeEnter
        | ToolRenderKind::WorktreeList
        | ToolRenderKind::WorktreeExit => {
            if let Some(details) = worktree_output_details(tool_name, structured) {
                return details;
            }
        }
        ToolRenderKind::FileMutation => {
            if let Some(detail_lines) =
                file_mutation_output_details(tool_name, output_preview, structured)
            {
                return detail_lines;
            }
        }
        ToolRenderKind::SendInput
        | ToolRenderKind::SpawnAgent
        | ToolRenderKind::WaitAgent
        | ToolRenderKind::ResumeAgent
        | ToolRenderKind::CloseAgent
        | ToolRenderKind::Generic => {}
    }

    if let Some(structured) = structured
        && let Some(details) = generic_structured_output_details(structured)
    {
        return details;
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
    let render_kind = ToolRenderKind::classify(tool_name);
    if render_kind != ToolRenderKind::FileMutation && render_kind != ToolRenderKind::NotebookEdit {
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
            label: "Stdout".to_string(),
            kind: ToolDetailBlockKind::Stdout,
            lines: stdout_preview,
        });
    }
    if has_stderr_preview {
        detail_lines.push(ToolDetail::NamedBlock {
            label: "Stderr".to_string(),
            kind: ToolDetailBlockKind::Stderr,
            lines: stderr_preview,
        });
    }

    if !has_stdout_preview && !has_stderr_preview && detail_lines.is_empty() {
        detail_lines.extend(generic_output_details(output_preview));
    }

    detail_lines
}

fn monitor_output_details(tool_name: &str, structured: Option<&Value>) -> Option<Vec<ToolDetail>> {
    let structured = structured?;
    match ToolRenderKind::classify(tool_name) {
        ToolRenderKind::MonitorStart | ToolRenderKind::MonitorStop => {
            let monitor = structured.get("monitor")?;
            Some(single_monitor_output_details(monitor))
        }
        ToolRenderKind::MonitorList => {
            let monitors = structured.get("monitors")?.as_array()?;
            let mut details = vec![ToolDetail::LabeledValue {
                label: ToolDetailLabel::Result,
                value: format!("{} monitor(s)", monitors.len()),
            }];
            let lines = monitors
                .iter()
                .filter_map(render_monitor_summary_line)
                .collect::<Vec<_>>();
            if !lines.is_empty() {
                details.push(ToolDetail::LabeledBlock {
                    label: ToolDetailLabel::Output,
                    lines,
                });
            }
            Some(details)
        }
        _ => None,
    }
}

fn code_diagnostics_output_details(structured: Option<&Value>) -> Option<Vec<ToolDetail>> {
    let structured = structured?;
    let result_count = structured
        .get("result_count")
        .and_then(Value::as_u64)
        .or_else(|| {
            structured
                .get("diagnostics")
                .and_then(Value::as_array)
                .map(|diagnostics| diagnostics.len() as u64)
        })
        .unwrap_or(0);
    let mut details = vec![ToolDetail::LabeledValue {
        label: ToolDetailLabel::Result,
        value: format!("{result_count} diagnostic(s)"),
    }];

    if let Some(path) = structured
        .get("requested_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Context,
            value: path.to_string(),
        });
    } else if structured
        .get("scope")
        .and_then(Value::as_str)
        .is_some_and(|scope| scope == "workspace")
    {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Context,
            value: "workspace".to_string(),
        });
    }

    if let Some(backend) = structured
        .get("backend")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::State,
            value: backend.to_string(),
        });
    }

    let lines = structured
        .get("diagnostics")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(render_code_diagnostic_line)
        .collect::<Vec<_>>();
    if !lines.is_empty() {
        details.push(ToolDetail::LabeledBlock {
            label: ToolDetailLabel::Output,
            lines,
        });
    }
    Some(details)
}

fn browser_output_details(structured: Option<&Value>) -> Option<Vec<ToolDetail>> {
    let browser = structured?.get("browser")?;
    let mut details = Vec::new();
    if let Some(browser_id) = browser
        .get("browser_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Session,
            value: browser_id.to_string(),
        });
    }
    if let Some(url) = browser
        .get("current_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Context,
            value: truncate_inline(url, 88),
        });
    }

    let mut state_parts = Vec::new();
    if let Some(status) = browser
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        state_parts.push(status.to_string());
    }
    state_parts.push(
        if browser
            .get("headless")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        {
            "headless".to_string()
        } else {
            "headful".to_string()
        },
    );
    if let Some(viewport) = browser.get("viewport").and_then(Value::as_object) {
        let width = viewport.get("width").and_then(Value::as_u64).unwrap_or(0);
        let height = viewport.get("height").and_then(Value::as_u64).unwrap_or(0);
        if width > 0 && height > 0 {
            state_parts.push(format!("{width}x{height}"));
        }
    }
    if !state_parts.is_empty() {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::State,
            value: state_parts.join(" · "),
        });
    }

    if let Some(title) = browser
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Result,
            value: truncate_inline(title, 88),
        });
    }

    Some(details)
}

fn code_search_output_details(structured: Option<&Value>) -> Option<Vec<ToolDetail>> {
    let structured = structured?;
    let result_count = structured
        .get("result_count")
        .and_then(Value::as_u64)
        .or_else(|| {
            structured
                .get("matches")
                .and_then(Value::as_array)
                .map(|matches| matches.len() as u64)
        })
        .unwrap_or(0);
    let mut details = vec![ToolDetail::LabeledValue {
        label: ToolDetailLabel::Result,
        value: format!("{result_count} match(es)"),
    }];

    if let Some(path_prefix) = structured
        .get("requested_path_prefix")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Context,
            value: path_prefix.to_string(),
        });
    } else {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Context,
            value: "workspace".to_string(),
        });
    }

    if let Some(backend) = structured
        .get("backend")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::State,
            value: backend.to_string(),
        });
    }

    let lines = structured
        .get("matches")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(6)
        .filter_map(render_code_search_match_summary)
        .collect::<Vec<_>>();
    if !lines.is_empty() {
        details.push(ToolDetail::LabeledBlock {
            label: ToolDetailLabel::Output,
            lines,
        });
    }

    Some(details)
}

fn cron_output_details(tool_name: &str, structured: Option<&Value>) -> Option<Vec<ToolDetail>> {
    let structured = structured?;
    match ToolRenderKind::classify(tool_name) {
        ToolRenderKind::CronCreate | ToolRenderKind::CronDelete => {
            let cron = structured.get("cron")?;
            Some(single_cron_output_details(cron))
        }
        ToolRenderKind::CronList => {
            let crons = structured.get("crons")?.as_array()?;
            let result_count = structured
                .get("result_count")
                .and_then(Value::as_u64)
                .unwrap_or(crons.len() as u64);
            let mut details = vec![ToolDetail::LabeledValue {
                label: ToolDetailLabel::Result,
                value: format!("{result_count} automation(s)"),
            }];
            let lines = crons
                .iter()
                .filter_map(render_cron_compact_summary_line)
                .collect::<Vec<_>>();
            if !lines.is_empty() {
                details.push(ToolDetail::LabeledBlock {
                    label: ToolDetailLabel::Output,
                    lines,
                });
            }
            Some(details)
        }
        _ => None,
    }
}

fn single_cron_output_details(cron: &Value) -> Vec<ToolDetail> {
    let cron_id = cron
        .get("cron_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("<unknown>");
    let mut details = vec![ToolDetail::LabeledValue {
        label: ToolDetailLabel::Result,
        value: cron_id.to_string(),
    }];

    if let Some(schedule) = render_cron_schedule_output(cron) {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Context,
            value: schedule,
        });
    }
    if let Some(status) = cron
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::State,
            value: status.to_string(),
        });
    }
    if let Some(summary) = cron
        .get("prompt_summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Effect,
            value: truncate_inline(summary, 96),
        });
    }
    details
}

fn notebook_edit_output_details(structured: Option<&Value>) -> Option<Vec<ToolDetail>> {
    let structured = structured?;
    let kind = structured
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("success");
    let mut details = Vec::new();

    if let Some(summary) = structured
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(ToolDetail::LabeledValue {
            label: if kind == "error" {
                ToolDetailLabel::Result
            } else {
                ToolDetailLabel::Effect
            },
            value: inline_preview_text(summary, 96),
        });
    }

    if let Some(path) = structured
        .get("requested_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Context,
            value: path.to_string(),
        });
    }

    if let Some(snapshot) = render_notebook_snapshot_summary(structured) {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Snapshot,
            value: snapshot,
        });
    }

    let operation_lines = structured
        .get("operations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(4)
        .filter_map(render_notebook_applied_edit_summary)
        .collect::<Vec<_>>();
    if !operation_lines.is_empty() {
        details.push(ToolDetail::LabeledBlock {
            label: ToolDetailLabel::Output,
            lines: operation_lines,
        });
    }

    let changed_cells = structured
        .get("changed_cells")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(3)
        .flat_map(render_notebook_cell_summary_lines)
        .collect::<Vec<_>>();
    if !changed_cells.is_empty() {
        details.push(ToolDetail::LabeledBlock {
            label: ToolDetailLabel::Files,
            lines: changed_cells,
        });
    }

    if kind == "error"
        && let Some(operation_count) = structured.get("operation_count").and_then(Value::as_u64)
    {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::State,
            value: format!("{operation_count} pending operation(s)"),
        });
    }

    Some(details)
}

fn notebook_read_output_details(structured: Option<&Value>) -> Option<Vec<ToolDetail>> {
    let structured = structured?;
    let output_cells = structured
        .get("output_cells")
        .and_then(Value::as_u64)
        .or_else(|| {
            structured
                .get("cells")
                .and_then(Value::as_array)
                .map(|cells| cells.len() as u64)
        })
        .unwrap_or(0);
    let mut details = vec![ToolDetail::LabeledValue {
        label: ToolDetailLabel::Result,
        value: format!("{output_cells} cell(s)"),
    }];

    if let Some(path) = structured
        .get("requested_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Context,
            value: path.to_string(),
        });
    }

    let mut state_parts = Vec::new();
    if let Some(language_name) = structured
        .get("language_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        state_parts.push(language_name.to_string());
    }
    if let Some(kernelspec_name) = structured
        .get("kernelspec_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        state_parts.push(format!("kernel {kernelspec_name}"));
    }
    if let Some(nbformat) = structured.get("nbformat").and_then(Value::as_u64) {
        let minor = structured
            .get("nbformat_minor")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        state_parts.push(format!("nbformat {nbformat}.{minor}"));
    }
    if !state_parts.is_empty() {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::State,
            value: state_parts.join(" · "),
        });
    }

    let lines = structured
        .get("cells")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(4)
        .flat_map(render_notebook_cell_summary_lines)
        .collect::<Vec<_>>();
    if !lines.is_empty() {
        details.push(ToolDetail::LabeledBlock {
            label: ToolDetailLabel::Output,
            lines,
        });
    }
    Some(details)
}

fn render_notebook_snapshot_summary(structured: &Value) -> Option<String> {
    let before = structured
        .get("snapshot_before")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let after = structured
        .get("snapshot_after")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match (before, after) {
        (Some(before), Some(after)) => Some(format!(
            "{} -> {}",
            truncate_inline(before, 16),
            truncate_inline(after, 16)
        )),
        (Some(before), None) => Some(truncate_inline(before, 16)),
        _ => None,
    }
}

fn render_cron_schedule_argument(schedule: Option<&Value>) -> Option<String> {
    let schedule = schedule?;
    let kind = schedule
        .get("kind")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    match kind {
        "once_after" => schedule
            .get("delay_seconds")
            .and_then(Value::as_u64)
            .map(|delay| format!("once after {delay}s")),
        "every_seconds" => {
            let interval_seconds = schedule.get("interval_seconds").and_then(Value::as_u64)?;
            let mut line = format!("every {interval_seconds}s");
            if let Some(start_after) = schedule.get("start_after_seconds").and_then(Value::as_u64) {
                line.push_str(&format!(", start after {start_after}s"));
            }
            if let Some(max_runs) = schedule.get("max_runs").and_then(Value::as_u64) {
                line.push_str(&format!(", max {max_runs} run(s)"));
            }
            Some(line)
        }
        _ => None,
    }
}

fn render_cron_schedule_output(cron: &Value) -> Option<String> {
    let schedule = cron.get("schedule")?;
    let kind = schedule
        .get("kind")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    match kind {
        "once" => schedule
            .get("run_at_unix_s")
            .and_then(Value::as_u64)
            .map(|run_at| format!("once at {run_at}")),
        "recurring" => {
            let interval_seconds = schedule.get("interval_seconds").and_then(Value::as_u64)?;
            let next_run_unix_s = schedule.get("next_run_unix_s").and_then(Value::as_u64)?;
            let mut line = format!("every {interval_seconds}s, next at {next_run_unix_s}");
            if let Some(max_runs) = schedule.get("max_runs").and_then(Value::as_u64) {
                line.push_str(&format!(", max {max_runs} run(s)"));
            }
            Some(line)
        }
        _ => None,
    }
}

fn render_cron_compact_summary_line(cron: &Value) -> Option<String> {
    let cron_id = cron
        .get("cron_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let status = cron
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown");
    let schedule =
        render_cron_schedule_output(cron).unwrap_or_else(|| "schedule unknown".to_string());
    let summary = cron
        .get("prompt_summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|summary| truncate_inline(summary, 72))
        .unwrap_or_else(|| "<empty>".to_string());
    Some(format!("{cron_id} {status} · {schedule} · {summary}"))
}

fn render_notebook_applied_edit_summary(operation: &Value) -> Option<String> {
    let kind = operation
        .get("kind")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let cell_index = operation
        .get("cell_index")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cell_type = operation
        .get("cell_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("cell");
    Some(match kind {
        "replace_cell" => format!("Replace cell {cell_index} as {cell_type}"),
        "insert_cell" => format!("Insert {cell_type} cell at {cell_index}"),
        "delete_cell" => format!("Delete cell {cell_index}"),
        _ => return None,
    })
}

fn render_notebook_edit_operation_preview_lines(
    operations: &[Value],
    max_operations: usize,
) -> Vec<String> {
    if operations.is_empty() {
        return Vec::new();
    }

    let keep = max_operations.max(1);
    let mut lines = operations
        .iter()
        .take(keep)
        .filter_map(render_notebook_edit_argument_preview)
        .collect::<Vec<_>>();
    let hidden = operations.len().saturating_sub(lines.len());
    if hidden > 0 {
        lines.push(format!("… +{hidden} operation(s)"));
    }
    lines
}

fn render_notebook_edit_argument_preview(operation: &Value) -> Option<String> {
    let kind = operation
        .get("operation")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let cell_index = operation
        .get("cell_index")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cell_type = operation
        .get("cell_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("cell");
    Some(match kind {
        "replace_cell" => format!("Replace cell {cell_index} as {cell_type}"),
        "insert_cell" => format!("Insert {cell_type} cell at {cell_index}"),
        "delete_cell" => format!("Delete cell {cell_index}"),
        _ => return None,
    })
}

fn render_notebook_cell_summary_lines(cell: &Value) -> Vec<String> {
    let index = cell.get("index").and_then(Value::as_u64).unwrap_or(0);
    let cell_type = cell
        .get("cell_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown");
    let source_line_count = cell
        .get("source_line_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output_count = cell
        .get("output_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut header = format!("[{cell_type} #{index}] {source_line_count} line(s)");
    if let Some(execution_count) = cell.get("execution_count").and_then(Value::as_i64) {
        header.push_str(&format!(" exec={execution_count}"));
    }
    if output_count > 0 {
        header.push_str(&format!(", {output_count} output(s)"));
    }

    let mut lines = vec![header];
    if let Some(source_preview) = cell.get("source_preview").and_then(Value::as_array) {
        let preview = source_preview
            .iter()
            .filter_map(Value::as_str)
            .take(2)
            .map(|line| truncate_inline(line, 88))
            .collect::<Vec<_>>();
        if !preview.is_empty() {
            lines.push(format!("source {}", preview.join(" | ")));
        }
    }
    if let Some(outputs) = cell.get("outputs").and_then(Value::as_array) {
        for output in outputs.iter().take(2) {
            let kind = output
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("output");
            let summary = output
                .get("summary")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| truncate_inline(value, 88))
                .unwrap_or_else(|| "<output>".to_string());
            lines.push(format!("{kind} {summary}"));
        }
    }
    lines
}

fn render_code_search_match_summary(entry: &Value) -> Option<String> {
    let path = entry.get("location")?.get("path")?.as_str()?.trim();
    let line = entry.get("location")?.get("line")?.as_u64()?;
    let column = entry.get("location")?.get("column")?.as_u64()?;
    let score_suffix = entry
        .get("score")
        .and_then(Value::as_u64)
        .map(|score| format!(" · score {score}"))
        .unwrap_or_default();
    match entry.get("kind")?.as_str()? {
        "symbol" => {
            let symbol_name = entry
                .get("symbol_name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("<symbol>");
            let symbol_kind = entry
                .get("symbol_kind")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("symbol");
            Some(format!(
                "[{symbol_kind}] {path}:{line}:{column} {symbol_name}{score_suffix}"
            ))
        }
        "text" => {
            let preview = entry
                .get("line_text")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| truncate_inline(value, 88))
                .unwrap_or_else(|| "<snippet>".to_string());
            Some(format!("{path}:{line}:{column} {preview}{score_suffix}"))
        }
        _ => None,
    }
}

fn worktree_output_details(tool_name: &str, structured: Option<&Value>) -> Option<Vec<ToolDetail>> {
    let structured = structured?;
    match ToolRenderKind::classify(tool_name) {
        ToolRenderKind::WorktreeEnter | ToolRenderKind::WorktreeExit => {
            let worktree = structured.get("worktree")?;
            Some(single_worktree_output_details(worktree))
        }
        ToolRenderKind::WorktreeList => {
            let worktrees = structured.get("worktrees")?.as_array()?;
            let mut details = vec![ToolDetail::LabeledValue {
                label: ToolDetailLabel::Result,
                value: format!("{} worktree(s)", worktrees.len()),
            }];
            let lines = worktrees
                .iter()
                .filter_map(render_worktree_summary_line)
                .collect::<Vec<_>>();
            if !lines.is_empty() {
                details.push(ToolDetail::LabeledBlock {
                    label: ToolDetailLabel::Output,
                    lines,
                });
            }
            Some(details)
        }
        _ => None,
    }
}

fn single_monitor_output_details(monitor: &Value) -> Vec<ToolDetail> {
    let mut details = Vec::new();
    if let Some(monitor_id) = monitor
        .get("monitor_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Session,
            value: monitor_id.to_string(),
        });
    }
    if let Some(status) = monitor
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::State,
            value: status.to_string(),
        });
    }
    if let Some(cwd) = monitor
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Context,
            value: cwd.to_string(),
        });
    }
    if let Some(command) = monitor
        .get("command")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Output,
            value: inline_preview_text(command, 96),
        });
    }
    details
}

fn single_worktree_output_details(worktree: &Value) -> Vec<ToolDetail> {
    let mut details = Vec::new();
    if let Some(worktree_id) = worktree
        .get("worktree_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Session,
            value: worktree_id.to_string(),
        });
    }
    if let Some(status) = worktree
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::State,
            value: status.to_string(),
        });
    }
    if let Some(root) = worktree
        .get("root")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Context,
            value: root.to_string(),
        });
    }
    if let Some(scope) = worktree
        .get("scope")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Note,
            value: format!("scope {scope}"),
        });
    }
    if let Some(label) = worktree
        .get("label")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(ToolDetail::LabeledValue {
            label: ToolDetailLabel::Note,
            value: format!("label {}", inline_preview_text(label, 72)),
        });
    }
    details
}

fn render_monitor_summary_line(monitor: &Value) -> Option<String> {
    let monitor_id = monitor
        .get("monitor_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let status = monitor
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown");
    let command = monitor
        .get("command")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("<empty>");
    Some(format!(
        "{} {} · {}",
        monitor_id,
        status,
        inline_preview_text(command, 72)
    ))
}

fn render_worktree_summary_line(worktree: &Value) -> Option<String> {
    let worktree_id = worktree
        .get("worktree_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let status = worktree
        .get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown");
    let root = worktree
        .get("root")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("<unknown>");
    let label = worktree
        .get("label")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    Some(match label {
        Some(label) => format!(
            "{} {} · {} · {}",
            worktree_id,
            status,
            inline_preview_text(root, 56),
            inline_preview_text(label, 24)
        ),
        None => format!(
            "{} {} · {}",
            worktree_id,
            status,
            inline_preview_text(root, 72)
        ),
    })
}

fn render_code_diagnostic_line(diagnostic: &Value) -> Option<String> {
    let severity = diagnostic
        .get("severity")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown");
    let message = diagnostic
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("<empty>");
    let location = diagnostic.get("location")?;
    let path = location
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("<unknown>");
    let line = location
        .get("line")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let column = location
        .get("column")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let mut rendered = format!("[{severity}] {path}:{line}:{column} {message}");
    if let Some(provider) = diagnostic
        .get("provider")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        rendered.push_str(&format!(" · {provider}"));
    }
    Some(rendered)
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

    if let Ok(parsed) = serde_json::from_str::<Value>(trimmed)
        && let Some(details) = generic_structured_output_details(&parsed)
    {
        return details;
    }

    let collapsed = collapse_preview_text(output_preview, 8, 120, PreviewCollapse::HeadTail);
    if collapsed.len() > 1
        || output_preview.chars().count() > 96
        || output_preview.lines().count() > 1
    {
        return vec![ToolDetail::LabeledBlock {
            label: ToolDetailLabel::Output,
            lines: collapsed,
        }];
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

fn summarize_generic_argument_object(object: &Map<String, Value>) -> Vec<String> {
    if object.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let mut used_primary_key = None;
    for (key, prefix) in [
        ("query", "Search"),
        ("prompt", "Prompt"),
        ("message", "Message"),
        ("path", "Path"),
        ("uri", "URI"),
        ("url", "URL"),
        ("target", "Target"),
        ("session_id", "Session"),
        ("id", "ID"),
    ] {
        if let Some(value) = object.get(key).and_then(Value::as_str).map(str::trim)
            && !value.is_empty()
        {
            lines.push(format!("{prefix} {}", inline_preview_text(value, 88)));
            used_primary_key = Some(key);
            break;
        }
    }

    let mut keys = object.keys().cloned().collect::<Vec<_>>();
    keys.sort_by(|left, right| {
        field_rank(left.as_str())
            .cmp(&field_rank(right.as_str()))
            .then_with(|| left.cmp(right))
    });
    for key in keys {
        if Some(key.as_str()) == used_primary_key {
            continue;
        }
        if let Some(value) = object.get(&key) {
            lines.extend(summarize_named_value_lines(
                &humanize_json_key(&key),
                value,
                2,
            ));
        }
        if lines.len() >= 4 {
            break;
        }
    }
    if lines.len() > 4 {
        lines.truncate(4);
    }
    lines
}

fn summarize_json_preview_lines(value: &Value, max_lines: usize) -> Option<Vec<String>> {
    let lines = match value {
        Value::Object(object) => {
            let mut keys = object.keys().cloned().collect::<Vec<_>>();
            keys.sort_by(|left, right| {
                field_rank(left.as_str())
                    .cmp(&field_rank(right.as_str()))
                    .then_with(|| left.cmp(right))
            });
            let mut lines = Vec::new();
            for key in keys.into_iter().take(max_lines) {
                if let Some(entry) = object.get(&key) {
                    lines.extend(summarize_named_value_lines(
                        &humanize_json_key(&key),
                        entry,
                        2,
                    ));
                }
                if lines.len() >= max_lines {
                    break;
                }
            }
            let remaining = object.len().saturating_sub(lines.len());
            if remaining > 0 {
                lines.push(format!("… {remaining} more field(s)"));
            }
            lines
        }
        Value::Array(items) => summarize_array_value_lines("Items", items),
        Value::String(text) => {
            collapse_preview_text(text, max_lines, 96, PreviewCollapse::HeadTail)
        }
        Value::Number(number) => vec![number.to_string()],
        Value::Bool(boolean) => vec![boolean.to_string()],
        Value::Null => Vec::new(),
    };
    (!lines.is_empty()).then_some(lines)
}

fn generic_structured_output_details(value: &Value) -> Option<Vec<ToolDetail>> {
    let lines = summarize_json_preview_lines(value, 6)?;
    Some(if lines.len() == 1 {
        vec![ToolDetail::LabeledValue {
            label: ToolDetailLabel::Output,
            value: lines[0].clone(),
        }]
    } else {
        vec![ToolDetail::LabeledBlock {
            label: ToolDetailLabel::Output,
            lines,
        }]
    })
}

fn summarize_named_value_lines(label: &str, value: &Value, max_lines: usize) -> Vec<String> {
    match value {
        Value::Null => Vec::new(),
        Value::String(text) => prefixed_collapse_lines(label, text, max_lines),
        Value::Number(number) => vec![format!("{label} {number}")],
        Value::Bool(boolean) => vec![format!("{label} {boolean}")],
        Value::Array(items) => summarize_array_value_lines(label, items),
        Value::Object(object) => {
            if let Some(summary) = summarize_object_value(object) {
                vec![format!("{label} {summary}")]
            } else {
                vec![format!("{label} {} field(s)", object.len())]
            }
        }
    }
}

fn summarize_array_value_lines(label: &str, items: &[Value]) -> Vec<String> {
    if items.is_empty() {
        return vec![format!("{label} 0 item(s)")];
    }
    let scalar_items = items
        .iter()
        .filter_map(scalar_json_preview)
        .take(3)
        .collect::<Vec<_>>();
    if scalar_items.len() == items.len() && !scalar_items.is_empty() {
        return vec![format!("{label} {}", scalar_items.join(" · "))];
    }
    vec![format!("{label} {} item(s)", items.len())]
}

fn summarize_object_value(object: &Map<String, Value>) -> Option<String> {
    for key in [
        "summary", "message", "name", "title", "path", "url", "state", "status", "id",
    ] {
        if let Some(value) = object.get(key).and_then(scalar_json_preview) {
            return Some(value);
        }
    }
    (!object.is_empty()).then(|| format!("{} field(s)", object.len()))
}

fn scalar_json_preview(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| inline_preview_text(trimmed, 72))
        }
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(boolean) => Some(boolean.to_string()),
        _ => None,
    }
}

fn prefixed_collapse_lines(label: &str, value: &str, max_lines: usize) -> Vec<String> {
    let collapsed = collapse_preview_text(value.trim(), max_lines, 96, PreviewCollapse::HeadTail);
    let Some((first, rest)) = collapsed.split_first() else {
        return Vec::new();
    };
    let mut lines = vec![format!("{label} {first}")];
    lines.extend(rest.iter().cloned());
    lines
}

fn humanize_json_key(key: &str) -> String {
    key.split(['_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let upper = part.to_ascii_uppercase();
            if matches!(
                upper.as_str(),
                "URL" | "URI" | "ID" | "CWD" | "STDOUT" | "STDERR"
            ) {
                upper
            } else {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn field_rank(key: &str) -> u8 {
    match key {
        "status" | "state" | "result" | "summary" => 0,
        "query" | "prompt" | "message" => 1,
        "path" | "uri" | "url" | "target" | "session_id" | "id" => 2,
        "cwd" | "workdir" | "timeout_ms" | "model" | "reasoning_effort" => 3,
        _ => 4,
    }
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
    fn code_diagnostics_arguments_render_scope_preview() {
        assert_eq!(
            tool_arguments_preview_lines(
                "cron_create",
                &json!({
                    "schedule": {"kind": "every_seconds", "interval_seconds": 300, "start_after_seconds": 0, "max_runs": 3},
                    "summary": "Review nightly regression queue",
                    "role": "reviewer"
                }),
            ),
            vec![
                "Schedule automation",
                "Review nightly regression queue",
                "every 300s, start after 0s, max 3 run(s)",
                "role reviewer"
            ]
        );
        assert_eq!(
            tool_arguments_preview_lines("cron_list", &json!({})),
            vec!["List automations"]
        );
        assert_eq!(
            tool_arguments_preview_lines("cron_delete", &json!({"cron_id": "cron_1"})),
            vec!["Cancel automation cron_1"]
        );
        assert_eq!(
            tool_arguments_preview_lines(
                "notebook_edit",
                &json!({
                    "path": "analysis.ipynb",
                    "operations": [
                        {"operation": "replace_cell", "cell_index": 2, "cell_type": "markdown", "source": "# Updated"},
                        {"operation": "insert_cell", "cell_index": 3, "cell_type": "code", "source": "print('hi')"},
                        {"operation": "delete_cell", "cell_index": 4},
                        {"operation": "replace_cell", "cell_index": 5, "cell_type": "raw", "source": "notes"}
                    ],
                    "expected_snapshot": "abc123"
                }),
            ),
            vec![
                "Edit notebook analysis.ipynb",
                "Replace cell 2 as markdown",
                "Insert code cell at 3",
                "Delete cell 4",
                "… +1 operation(s)",
                "snapshot guard enabled"
            ]
        );
        assert_eq!(
            tool_arguments_preview_lines("notebook_read", &json!({"path": "analysis.ipynb"})),
            vec!["Read notebook analysis.ipynb"]
        );
        assert_eq!(
            tool_arguments_preview_lines(
                "notebook_read",
                &json!({"path": "analysis.ipynb", "start_cell": 3, "cell_count": 2, "include_outputs": false})
            ),
            vec![
                "Read notebook analysis.ipynb",
                "start cell 3, count 2",
                "outputs hidden"
            ]
        );
        assert_eq!(
            tool_arguments_preview_lines("code_search", &json!({"query": "Engine"})),
            vec!["Search code for Engine"]
        );
        assert_eq!(
            tool_arguments_preview_lines(
                "code_search",
                &json!({"query": "Engine", "path_prefix": "src/runtime"})
            ),
            vec!["Search code for Engine in src/runtime"]
        );
        assert_eq!(
            tool_arguments_preview_lines("code_diagnostics", &json!({})),
            vec!["Inspect workspace diagnostics"]
        );
        assert_eq!(
            tool_arguments_preview_lines("code_diagnostics", &json!({"path": "src/lib.rs"})),
            vec!["Inspect diagnostics for src/lib.rs"]
        );
        assert_eq!(
            tool_arguments_preview_lines(
                "browser_open",
                &json!({
                    "url": "https://example.com",
                    "headless": false,
                    "viewport": {"width": 1280, "height": 720}
                })
            ),
            vec![
                "Open browser https://example.com",
                "mode headful",
                "viewport 1280x720"
            ]
        );
    }

    #[test]
    fn code_diagnostics_output_renders_typed_summary() {
        let cron_rendered = tool_output_detail_lines(
            "cron_create",
            "",
            Some(&json!({
                "cron": {
                    "cron_id": "cron_123",
                    "status": "scheduled",
                    "prompt_summary": "Review nightly regression queue",
                    "schedule": {"kind": "recurring", "interval_seconds": 300, "next_run_unix_s": 42, "max_runs": 3}
                }
            })),
        );

        assert_eq!(cron_rendered[0], "  └ Result cron_123");
        assert!(
            cron_rendered
                .iter()
                .any(|line| line == "  └ Context every 300s, next at 42, max 3 run(s)")
        );
        assert!(
            cron_rendered
                .iter()
                .any(|line| line == "  └ State scheduled")
        );

        let browser_rendered = tool_output_detail_lines(
            "browser_open",
            "",
            Some(&json!({
                "browser": {
                    "browser_id": "browser_123",
                    "status": "open",
                    "current_url": "https://example.com/app",
                    "headless": false,
                    "title": "Example App",
                    "viewport": {"width": 1280, "height": 720}
                }
            })),
        );
        assert_eq!(browser_rendered[0], "  └ Session browser_123");
        assert!(
            browser_rendered
                .iter()
                .any(|line| line == "  └ Context https://example.com/app")
        );
        assert!(
            browser_rendered
                .iter()
                .any(|line| line == "  └ State open · headful · 1280x720")
        );
        assert!(
            browser_rendered
                .iter()
                .any(|line| line == "  └ Result Example App")
        );

        let cron_list_rendered = tool_output_detail_lines(
            "cron_list",
            "",
            Some(&json!({
                "result_count": 2,
                "crons": [
                    {
                        "cron_id": "cron_1",
                        "status": "scheduled",
                        "prompt_summary": "Review nightly regression queue",
                        "schedule": {"kind": "recurring", "interval_seconds": 300, "next_run_unix_s": 42, "max_runs": 3}
                    },
                    {
                        "cron_id": "cron_2",
                        "status": "completed",
                        "prompt_summary": "Cleanup stale scratch files",
                        "schedule": {"kind": "once", "run_at_unix_s": 24}
                    }
                ]
            })),
        );

        assert_eq!(cron_list_rendered[0], "  └ Result 2 automation(s)");
        assert!(
            cron_list_rendered.iter().any(
                |line| line.contains("cron_1 scheduled · every 300s, next at 42, max 3 run(s)")
            )
        );
        assert!(cron_list_rendered.iter().any(|line| {
            line.contains("cron_2 completed · once at 24 · Cleanup stale scratch files")
        }));

        let cron_delete_rendered = tool_output_detail_lines(
            "cron_delete",
            "",
            Some(&json!({
                "cron": {
                    "cron_id": "cron_1",
                    "status": "cancelled",
                    "prompt_summary": "Review nightly regression queue",
                    "schedule": {"kind": "recurring", "interval_seconds": 300, "next_run_unix_s": 42, "max_runs": 3}
                }
            })),
        );

        assert_eq!(cron_delete_rendered[0], "  └ Result cron_1");
        assert!(
            cron_delete_rendered
                .iter()
                .any(|line| line == "  └ State cancelled")
        );

        let notebook_edit_rendered = tool_output_detail_lines(
            "notebook_edit",
            "",
            Some(&json!({
                "kind": "success",
                "requested_path": "analysis.ipynb",
                "summary": "Updated analysis.ipynb with 2 notebook operation(s)",
                "snapshot_before": "before123456",
                "snapshot_after": "after123456",
                "operations": [
                    {"kind": "replace_cell", "cell_index": 2, "cell_type": "markdown"},
                    {"kind": "insert_cell", "cell_index": 3, "cell_type": "code"}
                ],
                "changed_cells": [
                    {
                        "index": 2,
                        "cell_type": "markdown",
                        "source_line_count": 1,
                        "source_preview": ["# Updated"],
                        "output_count": 0
                    }
                ],
                "file_diffs": [
                    {"path": "analysis.ipynb", "preview": "@@ -1 +1 @@\n-old\n+new"}
                ]
            })),
        );

        assert_eq!(
            notebook_edit_rendered[0],
            "  └ Effect Updated analysis.ipynb with 2 notebook operation(s)"
        );
        assert!(
            notebook_edit_rendered
                .iter()
                .any(|line| line == "  └ Context analysis.ipynb")
        );
        assert!(
            notebook_edit_rendered
                .iter()
                .any(|line| line.contains("Snapshot before123456 -> after123456"))
        );
        assert!(
            notebook_edit_rendered
                .iter()
                .any(|line| line == "  └ Output Replace cell 2 as markdown")
        );

        let notebook_rendered = tool_output_detail_lines(
            "notebook_read",
            "",
            Some(&json!({
                "requested_path": "analysis.ipynb",
                "nbformat": 4,
                "nbformat_minor": 5,
                "language_name": "python",
                "kernelspec_name": "python3",
                "output_cells": 2,
                "cells": [
                    {
                        "index": 1,
                        "cell_type": "markdown",
                        "source_line_count": 2,
                        "source_preview": ["# Title", "Intro paragraph."],
                        "output_count": 0
                    },
                    {
                        "index": 2,
                        "cell_type": "code",
                        "execution_count": 3,
                        "source_line_count": 1,
                        "source_preview": ["print('hi')"],
                        "output_count": 1,
                        "outputs": [
                            {"kind": "stream", "summary": "stdout"}
                        ]
                    }
                ]
            })),
        );

        assert_eq!(notebook_rendered[0], "  └ Result 2 cell(s)");
        assert!(
            notebook_rendered
                .iter()
                .any(|line| line == "  └ Context analysis.ipynb")
        );
        assert!(
            notebook_rendered
                .iter()
                .any(|line| line == "  └ State python · kernel python3 · nbformat 4.5")
        );
        assert!(
            notebook_rendered
                .iter()
                .any(|line| line.contains("[markdown #1] 2 line(s)"))
        );
        assert!(
            notebook_rendered
                .iter()
                .any(|line| line.contains("source # Title | Intro paragraph."))
        );
        assert!(
            notebook_rendered
                .iter()
                .any(|line| line.contains("stream stdout"))
        );

        let search_rendered = tool_output_detail_lines(
            "code_search",
            "",
            Some(&json!({
                "query": "Engine",
                "requested_path_prefix": "src/runtime",
                "backend": "managed_lsp_with_text_fallback_v1",
                "result_count": 2,
                "matches": [
                    {
                        "score": 1000,
                        "kind": "symbol",
                        "location": {"path": "src/runtime.rs", "line": 10, "column": 12},
                        "symbol_name": "Engine",
                        "symbol_kind": "struct",
                        "line_text": "pub struct Engine;",
                        "signature": "pub struct Engine;"
                    },
                    {
                        "score": 560,
                        "kind": "text",
                        "location": {"path": "src/runtime.rs", "line": 22, "column": 9},
                        "line_text": "let _ = Engine {};"
                    }
                ]
            })),
        );

        assert_eq!(search_rendered[0], "  └ Result 2 match(es)");
        assert!(
            search_rendered
                .iter()
                .any(|line| line == "  └ Context src/runtime")
        );
        assert!(
            search_rendered
                .iter()
                .any(|line| line.contains("[struct] src/runtime.rs:10:12 Engine · score 1000"))
        );
        assert!(
            search_rendered
                .iter()
                .any(|line| line.contains("src/runtime.rs:22:9 let _ = Engine {}; · score 560"))
        );

        let rendered = tool_output_detail_lines(
            "code_diagnostics",
            "",
            Some(&json!({
                "scope": "path:src/lib.rs",
                "requested_path": "src/lib.rs",
                "backend": "managed_lsp_with_text_fallback_v1",
                "result_count": 1,
                "diagnostics": [
                    {
                        "location": {"path": "src/lib.rs", "line": 7, "column": 3},
                        "severity": "warning",
                        "message": "unused parameter",
                        "source": "lsp",
                        "provider": "rust-analyzer"
                    }
                ]
            })),
        );

        assert_eq!(rendered[0], "  └ Result 1 diagnostic(s)");
        assert!(rendered.iter().any(|line| line == "  └ Context src/lib.rs"));
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("[warning] src/lib.rs:7:3 unused parameter"))
        );
    }

    #[test]
    fn worktree_arguments_render_typed_preview() {
        assert_eq!(
            tool_arguments_preview_lines("worktree_enter", &json!({"label": "feature auth"})),
            vec!["Enter session worktree", "label feature auth"]
        );
        assert_eq!(
            tool_arguments_preview_lines("worktree_list", &json!({"include_inactive": true})),
            vec!["List worktrees including inactive"]
        );
        assert_eq!(
            tool_arguments_preview_lines("worktree_exit", &json!({"worktree_id": "worktree_123"})),
            vec!["Exit worktree worktree_123"]
        );
    }

    #[test]
    fn worktree_output_renders_typed_summary() {
        let rendered = tool_output_detail_lines(
            "worktree_enter",
            "",
            Some(&json!({
                "worktree": {
                    "worktree_id": "worktree_123",
                    "scope": "session",
                    "status": "active",
                    "root": "/tmp/nanoclaw-worktrees/feature-auth",
                    "label": "feature auth"
                }
            })),
        );

        assert!(
            rendered
                .iter()
                .any(|line| line == "  └ Session worktree_123")
        );
        assert!(rendered.iter().any(|line| line == "  └ State active"));
        assert!(
            rendered
                .iter()
                .any(|line| line == "  └ Context /tmp/nanoclaw-worktrees/feature-auth")
        );
        assert!(rendered.iter().any(|line| line == "  └ Note scope session"));
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
    fn spawn_agent_arguments_surface_dedicated_worktree_mode() {
        let rendered = tool_arguments_preview_lines(
            "spawn_agent",
            &json!({
                "agent_type": "reviewer",
                "worktree_mode": "dedicated",
            }),
        );

        assert_eq!(rendered[0], "spawn reviewer worktree=dedicated");
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
    fn generic_argument_preview_humanizes_structured_fields_without_raw_json() {
        let rendered = tool_arguments_preview_lines(
            "custom_tool",
            &json!({
                "query": "latest release",
                "url": "https://example.com/releases",
                "limit": 5
            }),
        );

        assert_eq!(rendered[0], "Search latest release");
        assert!(
            rendered
                .iter()
                .any(|line| line == "URL https://example.com/releases")
        );
        assert!(rendered.iter().any(|line| line == "Limit 5"));
        assert!(rendered.iter().all(|line| !line.contains('{')));
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

        assert!(rendered.contains("  └ Result exit 0"));
        assert!(rendered.contains("  └ Stdout"));
        assert!(rendered.contains("    ok"));
        assert!(!rendered.contains("```"));
    }

    #[test]
    fn generic_output_details_humanize_structured_json_without_dumping_raw_payloads() {
        let rendered = tool_output_detail_lines(
            "custom_tool",
            "{\"status\":\"ok\",\"url\":\"https://example.com/releases\",\"count\":3}",
            Some(&json!({
                "status": "ok",
                "url": "https://example.com/releases",
                "count": 3
            })),
        );

        assert!(rendered.iter().any(|line| line == "  └ Output Status ok"));
        assert!(
            rendered
                .iter()
                .any(|line| line == "    URL https://example.com/releases")
        );
        assert!(rendered.iter().any(|line| line == "    Count 3"));
        assert!(rendered.iter().all(|line| !line.contains('{')));
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
                .any(|line| line == "  └ Effect Wrote 18 bytes to src/lib.rs")
        );
        assert!(rendered.iter().any(|line| line == "  └ Files src/lib.rs"));
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

        assert!(rendered.iter().any(|line| line == "  └ Session exec_123"));
        assert!(rendered.iter().any(|line| line == "  └ Result exit 0"));
        assert!(rendered.iter().any(|line| line == "  └ Stdout"));
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

    #[test]
    fn notebook_edit_review_extracts_structured_diff_preview() {
        let review = tool_review(
            "notebook_edit",
            Some(&json!({
                "kind": "success",
                "summary": "Updated analysis.ipynb with 1 notebook operation(s)",
                "file_diffs": [{
                    "path": "analysis.ipynb",
                    "preview": "@@ -1 +1 @@\n-old\n+new"
                }]
            })),
        )
        .expect("expected review");

        assert_eq!(
            review.summary.as_deref(),
            Some("Updated analysis.ipynb with 1 notebook operation(s)")
        );
        assert_eq!(review.files.len(), 1);
        assert_eq!(review.files[0].path, "analysis.ipynb");
    }
}
