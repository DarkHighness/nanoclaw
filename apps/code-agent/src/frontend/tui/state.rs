use crate::backend::{
    PendingControlKind, PendingControlSummary, SessionPermissionMode, StartupDiagnosticsSnapshot,
};
use crate::statusline::{StatusLineConfig, StatusLineField, status_line_fields};
use crate::tool_render::{
    ToolDetail, ToolDetailBlockKind, preview_tool_details, serialize_tool_details,
};
use agent::types::MessageId;
use agent::types::TokenLedgerSnapshot;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, RwLock};
use std::time::Instant;

#[derive(Clone, Debug, Default)]
pub(crate) struct GitSnapshot {
    pub(crate) available: bool,
    pub(crate) repo_name: String,
    pub(crate) branch: String,
    pub(crate) staged: usize,
    pub(crate) modified: usize,
    pub(crate) untracked: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SessionSummary {
    pub(crate) workspace_name: String,
    pub(crate) active_session_ref: String,
    pub(crate) root_agent_session_id: String,
    pub(crate) provider_label: String,
    pub(crate) model: String,
    pub(crate) model_reasoning_effort: Option<String>,
    pub(crate) supported_model_reasoning_efforts: Vec<String>,
    pub(crate) workspace_root: PathBuf,
    pub(crate) git: GitSnapshot,
    pub(crate) tool_names: Vec<String>,
    pub(crate) store_label: String,
    pub(crate) store_warning: Option<String>,
    pub(crate) stored_session_count: usize,
    pub(crate) default_sandbox_summary: String,
    pub(crate) sandbox_summary: String,
    pub(crate) permission_mode: SessionPermissionMode,
    pub(crate) host_process_surfaces_allowed: bool,
    pub(crate) startup_diagnostics: StartupDiagnosticsSnapshot,
    pub(crate) queued_commands: usize,
    pub(crate) token_ledger: TokenLedgerSnapshot,
    pub(crate) statusline: StatusLineConfig,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum MainPaneMode {
    #[default]
    Transcript,
    View,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PlanEntry {
    pub(crate) id: String,
    pub(crate) content: String,
    pub(crate) status: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct StatusLinePickerState {
    pub(crate) selected: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ThinkingEffortPickerState {
    pub(crate) selected: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PendingControlPickerState {
    pub(crate) selected: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingControlEditorState {
    pub(crate) id: String,
    pub(crate) kind: PendingControlKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HistoryRollbackCandidate {
    pub(crate) message_id: MessageId,
    pub(crate) prompt: String,
    pub(crate) turn_preview_lines: Vec<TranscriptEntry>,
    pub(crate) removed_turn_count: usize,
    pub(crate) removed_message_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HistoryRollbackOverlayState {
    pub(crate) selected: usize,
    pub(crate) candidates: Vec<HistoryRollbackCandidate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HistoryRollbackState {
    Primed,
    Selecting(HistoryRollbackOverlayState),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TranscriptShellBlockKind {
    Stdout,
    Stderr,
    Diff,
}

impl From<ToolDetailBlockKind> for TranscriptShellBlockKind {
    fn from(value: ToolDetailBlockKind) -> Self {
        match value {
            ToolDetailBlockKind::Stdout => Self::Stdout,
            ToolDetailBlockKind::Stderr => Self::Stderr,
            ToolDetailBlockKind::Diff => Self::Diff,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TranscriptShellDetail {
    Command(String),
    Meta(String),
    TextBlock(Vec<String>),
    NamedBlock {
        label: String,
        kind: TranscriptShellBlockKind,
        lines: Vec<String>,
    },
    Raw {
        text: String,
        continuation: bool,
    },
}

impl TranscriptShellDetail {
    pub(crate) fn from_prefixed(raw: &str) -> Option<Self> {
        if let Some(detail) = raw.strip_prefix("  └ ") {
            return Some(Self::Raw {
                text: detail.to_string(),
                continuation: false,
            });
        }
        if let Some(detail) = raw.strip_prefix("    ") {
            return Some(Self::Raw {
                text: detail.to_string(),
                continuation: true,
            });
        }
        if raw.trim().is_empty() {
            return None;
        }
        Some(Self::Raw {
            text: raw.to_string(),
            continuation: false,
        })
    }

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
            Self::Raw { text, continuation } => {
                if *continuation {
                    vec![format!("    {text}")]
                } else {
                    vec![format!("  └ {text}")]
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TranscriptShellEntry {
    pub(crate) headline: String,
    pub(crate) detail_lines: Vec<TranscriptShellDetail>,
}

impl TranscriptShellEntry {
    pub(crate) fn new(
        headline: impl Into<String>,
        detail_lines: Vec<TranscriptShellDetail>,
    ) -> Self {
        Self {
            headline: headline.into(),
            detail_lines,
        }
    }

    fn from_body(body: &str) -> Self {
        let mut lines = body.lines();
        let headline = lines.next().unwrap_or_default().to_string();
        let remaining = lines.collect::<Vec<_>>();
        let mut detail_lines = Vec::new();
        let mut index = 0;

        while let Some(raw_line) = remaining.get(index).copied() {
            let Some(detail) = raw_line.strip_prefix("  └ ") else {
                if let Some(detail_line) = TranscriptShellDetail::from_prefixed(raw_line) {
                    detail_lines.push(detail_line);
                }
                index += 1;
                continue;
            };

            let mut block_lines = Vec::new();
            let mut next = index + 1;
            while let Some(continuation) = remaining.get(next).copied() {
                let Some(continuation) = continuation.strip_prefix("    ") else {
                    break;
                };
                block_lines.push(continuation.to_string());
                next += 1;
            }

            detail_lines.push(classify_shell_detail(detail, block_lines));
            index = next;
        }

        Self::new(headline, detail_lines)
    }

    pub(crate) fn serialized_lines(&self) -> Vec<String> {
        let mut lines = vec![self.headline.clone()];
        lines.extend(
            self.detail_lines
                .iter()
                .flat_map(TranscriptShellDetail::serialized_lines),
        );
        lines
    }

    pub(crate) fn preview_with_detail_lines(&self, max_lines: usize) -> Self {
        let mut remaining = max_lines;
        let mut detail_lines = Vec::new();
        for detail in &self.detail_lines {
            let visible_lines = detail.serialized_lines();
            if visible_lines.is_empty() {
                continue;
            }
            if remaining == 0 {
                break;
            }
            if visible_lines.len() <= remaining {
                detail_lines.push(detail.clone());
                remaining -= visible_lines.len();
                continue;
            }

            detail_lines.push(match detail {
                TranscriptShellDetail::TextBlock(lines) => TranscriptShellDetail::TextBlock(
                    lines.iter().take(remaining).cloned().collect(),
                ),
                TranscriptShellDetail::NamedBlock { label, kind, lines } => {
                    TranscriptShellDetail::NamedBlock {
                        label: label.clone(),
                        kind: *kind,
                        lines: lines
                            .iter()
                            .take(remaining.saturating_sub(1))
                            .cloned()
                            .collect(),
                    }
                }
                TranscriptShellDetail::Command(command) => {
                    TranscriptShellDetail::Command(command.clone())
                }
                TranscriptShellDetail::Meta(text) => TranscriptShellDetail::Meta(text.clone()),
                TranscriptShellDetail::Raw { text, continuation } => TranscriptShellDetail::Raw {
                    text: text.clone(),
                    continuation: *continuation,
                },
            });
            break;
        }

        Self::new(self.headline.clone(), detail_lines)
    }

    pub(crate) fn serialized_body(&self) -> String {
        self.serialized_lines().join("\n")
    }
}

fn classify_shell_detail(text: &str, block_lines: Vec<String>) -> TranscriptShellDetail {
    if text.starts_with("$ ") && block_lines.is_empty() {
        return TranscriptShellDetail::Command(text.to_string());
    }
    if block_lines.is_empty() && is_shell_meta_line(text) {
        return TranscriptShellDetail::Meta(text.to_string());
    }
    if let Some(kind) = named_block_kind(text) {
        return TranscriptShellDetail::NamedBlock {
            label: text.to_string(),
            kind,
            lines: block_lines,
        };
    }
    if block_lines.is_empty() {
        TranscriptShellDetail::Raw {
            text: text.to_string(),
            continuation: false,
        }
    } else {
        let mut lines = Vec::with_capacity(block_lines.len() + 1);
        lines.push(text.to_string());
        lines.extend(block_lines);
        TranscriptShellDetail::TextBlock(lines)
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

fn is_shell_meta_line(text: &str) -> bool {
    text.starts_with("exit ")
        || text == "timed out"
        || text.starts_with("snapshot ")
        || text.starts_with("reason ")
        || text.starts_with("origin ")
        || text == "cancelled"
}

fn named_block_kind(label: &str) -> Option<TranscriptShellBlockKind> {
    if label == "stdout" {
        Some(TranscriptShellBlockKind::Stdout)
    } else if label == "stderr" {
        Some(TranscriptShellBlockKind::Stderr)
    } else if label.starts_with("diff ") || label == "diff" {
        Some(TranscriptShellBlockKind::Diff)
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TranscriptToolStatus {
    Requested,
    WaitingApproval,
    Approved,
    Running,
    Finished,
    Denied,
    Failed,
    Cancelled,
}

impl TranscriptToolStatus {
    fn headline(self, tool_name: &str) -> String {
        match self {
            Self::Requested => format!("Requested {tool_name}"),
            Self::WaitingApproval => format!("Awaiting approval for {tool_name}"),
            Self::Approved => format!("Approved {tool_name}"),
            Self::Running => format!("Running {tool_name}"),
            Self::Finished => format!("Finished {tool_name}"),
            Self::Denied => format!("Denied {tool_name}"),
            Self::Failed => format!("{tool_name} failed"),
            Self::Cancelled => format!("Cancelled {tool_name}"),
        }
    }

    fn marker(self) -> &'static str {
        match self {
            Self::Approved => "✔",
            Self::Denied | Self::Failed | Self::Cancelled => "✗",
            Self::Requested | Self::WaitingApproval | Self::Running | Self::Finished => "•",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TranscriptToolEntry {
    pub(crate) status: TranscriptToolStatus,
    pub(crate) tool_name: String,
    pub(crate) headline: String,
    pub(crate) detail_lines: Vec<ToolDetail>,
}

impl TranscriptToolEntry {
    pub(crate) fn new(
        status: TranscriptToolStatus,
        tool_name: impl Into<String>,
        detail_lines: Vec<ToolDetail>,
    ) -> Self {
        let tool_name = tool_name.into();
        let headline = status.headline(&tool_name);
        Self {
            status,
            tool_name,
            headline,
            detail_lines,
        }
    }

    pub(crate) fn marker(&self) -> &'static str {
        self.status.marker()
    }

    pub(crate) fn serialized_lines(&self) -> Vec<String> {
        let mut lines = vec![self.headline.clone()];
        lines.extend(serialize_tool_details(&self.detail_lines));
        lines
    }

    pub(crate) fn serialized_body(&self) -> String {
        self.serialized_lines().join("\n")
    }

    pub(crate) fn preview_with_detail_lines(&self, max_lines: usize) -> Self {
        Self {
            status: self.status,
            tool_name: self.tool_name.clone(),
            headline: self.headline.clone(),
            detail_lines: preview_tool_details(&self.detail_lines, max_lines),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TranscriptPlanEntry {
    pub(crate) headline: String,
    pub(crate) explanation: Option<String>,
    pub(crate) items: Vec<PlanEntry>,
}

impl TranscriptPlanEntry {
    pub(crate) fn new(explanation: Option<String>, items: Vec<PlanEntry>) -> Self {
        Self {
            headline: "Updated Plan".to_string(),
            explanation: explanation
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            items,
        }
    }

    pub(crate) fn serialized_lines(&self) -> Vec<String> {
        let mut lines = vec![self.headline.clone()];
        if let Some(explanation) = &self.explanation {
            lines.push(format!("  └ {explanation}"));
        }
        if self.items.is_empty() {
            lines.push("  └ (no steps provided)".to_string());
        } else {
            lines.extend(self.items.iter().map(|item| {
                format!(
                    "  └ [{}] {}",
                    plan_status_marker(item.status.as_str()),
                    item.content
                )
            }));
        }
        lines
    }

    pub(crate) fn serialized_body(&self) -> String {
        self.serialized_lines().join("\n")
    }
}

fn plan_status_marker(status: &str) -> &'static str {
    match status {
        "completed" => "x",
        "in_progress" => "~",
        _ => " ",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TranscriptEntry {
    UserPrompt(String),
    AssistantMessage(String),
    Tool(TranscriptToolEntry),
    Plan(TranscriptPlanEntry),
    ShellSummary(TranscriptShellEntry),
    SuccessSummary(TranscriptShellEntry),
    ErrorSummary(TranscriptShellEntry),
    WarningSummary(TranscriptShellEntry),
}

impl TranscriptEntry {
    pub(crate) fn tool(
        status: TranscriptToolStatus,
        tool_name: impl Into<String>,
        detail_lines: Vec<ToolDetail>,
    ) -> Self {
        Self::Tool(TranscriptToolEntry::new(status, tool_name, detail_lines))
    }

    pub(crate) fn plan_update(explanation: Option<String>, items: Vec<PlanEntry>) -> Self {
        Self::Plan(TranscriptPlanEntry::new(explanation, items))
    }

    pub(crate) fn shell_summary_details(
        headline: impl Into<String>,
        detail_lines: Vec<TranscriptShellDetail>,
    ) -> Self {
        Self::ShellSummary(TranscriptShellEntry::new(headline, detail_lines))
    }

    pub(crate) fn success_summary_details(
        headline: impl Into<String>,
        detail_lines: Vec<TranscriptShellDetail>,
    ) -> Self {
        Self::SuccessSummary(TranscriptShellEntry::new(headline, detail_lines))
    }

    pub(crate) fn error_summary_details(
        headline: impl Into<String>,
        detail_lines: Vec<TranscriptShellDetail>,
    ) -> Self {
        Self::ErrorSummary(TranscriptShellEntry::new(headline, detail_lines))
    }

    pub(crate) fn append_text(&mut self, delta: &str) -> bool {
        match self {
            Self::UserPrompt(text) | Self::AssistantMessage(text) => {
                text.push_str(delta);
                true
            }
            Self::Tool(_)
            | Self::Plan(_)
            | Self::ShellSummary(_)
            | Self::SuccessSummary(_)
            | Self::ErrorSummary(_)
            | Self::WarningSummary(_) => false,
        }
    }

    pub(crate) fn serialized(&self) -> String {
        match self {
            Self::UserPrompt(text) => format!("› {text}"),
            Self::AssistantMessage(text) => format!("• {text}"),
            Self::Tool(entry) => format!("{} {}", entry.marker(), entry.serialized_body()),
            Self::Plan(entry) => format!("• {}", entry.serialized_body()),
            Self::ShellSummary(summary) => format!("• {}", summary.serialized_body()),
            Self::SuccessSummary(summary) => format!("✔ {}", summary.serialized_body()),
            Self::ErrorSummary(summary) => format!("✗ {}", summary.serialized_body()),
            Self::WarningSummary(summary) => format!("⚠ {}", summary.serialized_body()),
        }
    }

    pub(crate) fn marker(&self) -> &'static str {
        match self {
            Self::UserPrompt(_) => "›",
            Self::AssistantMessage(_) | Self::ShellSummary(_) => "•",
            Self::Tool(entry) => entry.marker(),
            Self::Plan(_) => "•",
            Self::SuccessSummary(_) => "✔",
            Self::ErrorSummary(_) => "✗",
            Self::WarningSummary(_) => "⚠",
        }
    }

    pub(crate) fn body(&self) -> &str {
        match self {
            Self::UserPrompt(text) | Self::AssistantMessage(text) => text,
            Self::Tool(entry) => entry.headline.as_str(),
            Self::Plan(entry) => entry.headline.as_str(),
            Self::ShellSummary(summary)
            | Self::SuccessSummary(summary)
            | Self::ErrorSummary(summary)
            | Self::WarningSummary(summary) => summary.headline.as_str(),
        }
    }

    pub(crate) fn shell_summary(&self) -> Option<&TranscriptShellEntry> {
        match self {
            Self::ShellSummary(summary)
            | Self::SuccessSummary(summary)
            | Self::ErrorSummary(summary)
            | Self::WarningSummary(summary) => Some(summary),
            Self::UserPrompt(_) | Self::AssistantMessage(_) | Self::Tool(_) | Self::Plan(_) => None,
        }
    }

    pub(crate) fn is_shell_summary(&self) -> bool {
        self.shell_summary().is_some()
    }

    pub(crate) fn tool_entry(&self) -> Option<&TranscriptToolEntry> {
        match self {
            Self::Tool(entry) => Some(entry),
            Self::UserPrompt(_)
            | Self::AssistantMessage(_)
            | Self::Plan(_)
            | Self::ShellSummary(_)
            | Self::SuccessSummary(_)
            | Self::ErrorSummary(_)
            | Self::WarningSummary(_) => None,
        }
    }

    pub(crate) fn plan_entry(&self) -> Option<&TranscriptPlanEntry> {
        match self {
            Self::Plan(entry) => Some(entry),
            Self::UserPrompt(_)
            | Self::AssistantMessage(_)
            | Self::Tool(_)
            | Self::ShellSummary(_)
            | Self::SuccessSummary(_)
            | Self::ErrorSummary(_)
            | Self::WarningSummary(_) => None,
        }
    }
}

impl std::fmt::Display for TranscriptEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.serialized())
    }
}

impl From<&str> for TranscriptEntry {
    fn from(value: &str) -> Self {
        Self::from(value.to_string())
    }
}

impl From<String> for TranscriptEntry {
    fn from(value: String) -> Self {
        if let Some(body) = value.strip_prefix("› ") {
            return Self::UserPrompt(body.to_string());
        }
        if let Some(body) = value.strip_prefix("✔ ") {
            return Self::SuccessSummary(TranscriptShellEntry::from_body(body));
        }
        if let Some(body) = value.strip_prefix("✗ ") {
            return Self::ErrorSummary(TranscriptShellEntry::from_body(body));
        }
        if let Some(body) = value.strip_prefix("⚠ ") {
            return Self::WarningSummary(TranscriptShellEntry::from_body(body));
        }
        if let Some(body) = value.strip_prefix("• ") {
            if body.lines().any(|line| line.starts_with("  └ ")) {
                return Self::ShellSummary(TranscriptShellEntry::from_body(body));
            }
            return Self::AssistantMessage(body.to_string());
        }
        Self::AssistantMessage(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InspectorEntry {
    Section(String),
    Field {
        key: String,
        value: String,
    },
    Transcript(TranscriptEntry),
    CollectionItem {
        primary: String,
        secondary: Option<String>,
    },
    Plain(String),
    Muted(String),
    Command(String),
    Empty,
}

impl InspectorEntry {
    pub(crate) fn section(title: impl Into<String>) -> Self {
        Self::Section(title.into())
    }

    pub(crate) fn field(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self::Field {
            key: key.into(),
            value: value.into(),
        }
    }

    pub(crate) fn transcript(entry: impl Into<TranscriptEntry>) -> Self {
        Self::Transcript(entry.into())
    }

    pub(crate) fn collection(
        primary: impl Into<String>,
        secondary: Option<impl Into<String>>,
    ) -> Self {
        Self::CollectionItem {
            primary: primary.into(),
            secondary: secondary.map(Into::into),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TuiState {
    pub(crate) session: SessionSummary,
    pub(crate) main_pane: MainPaneMode,
    pub(crate) show_tool_details: bool,
    pub(crate) input: String,
    pub(crate) command_completion_index: usize,
    pub(crate) transcript: Vec<TranscriptEntry>,
    pub(crate) transcript_scroll: u16,
    pub(crate) follow_transcript: bool,
    pub(crate) inspector_title: String,
    pub(crate) inspector: Vec<InspectorEntry>,
    pub(crate) inspector_scroll: u16,
    pub(crate) activity: Vec<String>,
    pub(crate) activity_scroll: u16,
    pub(crate) status: String,
    pub(crate) turn_running: bool,
    pub(crate) turn_started_at: Option<Instant>,
    pub(crate) active_tool_label: Option<String>,
    pub(crate) plan_items: Vec<PlanEntry>,
    pub(crate) pending_controls: Vec<PendingControlSummary>,
    pub(crate) pending_control_picker: Option<PendingControlPickerState>,
    pub(crate) editing_pending_control: Option<PendingControlEditorState>,
    pub(crate) statusline_picker: Option<StatusLinePickerState>,
    pub(crate) thinking_effort_picker: Option<ThinkingEffortPickerState>,
    pub(crate) history_rollback: Option<HistoryRollbackState>,
}

impl TuiState {
    pub(crate) fn show_main_view<I>(&mut self, title: impl Into<String>, lines: I)
    where
        I: IntoIterator<Item = InspectorEntry>,
    {
        self.main_pane = MainPaneMode::View;
        self.inspector_title = title.into();
        self.inspector = lines.into_iter().collect();
        self.inspector_scroll = 0;
        self.pending_control_picker = None;
        self.statusline_picker = None;
        self.thinking_effort_picker = None;
        self.history_rollback = None;
    }

    pub(crate) fn show_transcript_pane(&mut self) {
        self.main_pane = MainPaneMode::Transcript;
        self.pending_control_picker = None;
        self.statusline_picker = None;
        self.thinking_effort_picker = None;
        self.history_rollback = None;
    }

    pub(crate) fn open_statusline_picker(&mut self) {
        self.main_pane = MainPaneMode::View;
        self.inspector_title = "Status Line".to_string();
        self.inspector.clear();
        self.inspector_scroll = 0;
        self.pending_control_picker = None;
        self.thinking_effort_picker = None;
        self.history_rollback = None;
        self.statusline_picker
            .get_or_insert_with(StatusLinePickerState::default)
            .selected = 0;
    }

    pub(crate) fn open_thinking_effort_picker(&mut self) {
        self.main_pane = MainPaneMode::View;
        self.inspector_title = "Thinking Effort".to_string();
        self.inspector.clear();
        self.inspector_scroll = 0;
        self.pending_control_picker = None;
        self.statusline_picker = None;
        self.history_rollback = None;
        let selected = self
            .session
            .supported_model_reasoning_efforts
            .iter()
            .position(|level| {
                Some(level.as_str()) == self.session.model_reasoning_effort.as_deref()
            })
            .unwrap_or(0);
        self.thinking_effort_picker = Some(ThinkingEffortPickerState { selected });
    }

    pub(crate) fn close_statusline_picker(&mut self) {
        self.statusline_picker = None;
        self.show_transcript_pane();
    }

    pub(crate) fn close_thinking_effort_picker(&mut self) {
        self.thinking_effort_picker = None;
        self.show_transcript_pane();
    }

    pub(crate) fn open_pending_control_picker(&mut self, select_latest: bool) -> bool {
        if self.pending_controls.is_empty() {
            return false;
        }
        self.main_pane = MainPaneMode::Transcript;
        let selected = if select_latest {
            self.pending_controls.len().saturating_sub(1)
        } else {
            self.pending_control_picker
                .as_ref()
                .map(|picker| picker.selected)
                .unwrap_or_else(|| self.pending_controls.len().saturating_sub(1))
                .min(self.pending_controls.len().saturating_sub(1))
        };
        self.pending_control_picker = Some(PendingControlPickerState { selected });
        self.statusline_picker = None;
        self.thinking_effort_picker = None;
        self.history_rollback = None;
        true
    }

    pub(crate) fn close_pending_control_picker(&mut self) {
        self.pending_control_picker = None;
    }

    pub(crate) fn move_pending_control_picker(&mut self, backwards: bool) -> bool {
        let Some(picker) = self.pending_control_picker.as_mut() else {
            return false;
        };
        let total = self.pending_controls.len();
        if total == 0 {
            return false;
        }
        picker.selected = if backwards {
            picker.selected.checked_sub(1).unwrap_or(total - 1)
        } else {
            (picker.selected + 1) % total
        };
        true
    }

    pub(crate) fn selected_pending_control(&self) -> Option<PendingControlSummary> {
        let picker = self.pending_control_picker.as_ref()?;
        self.pending_controls.get(picker.selected).cloned()
    }

    pub(crate) fn begin_pending_control_edit(&mut self) -> Option<PendingControlSummary> {
        let selected = self.selected_pending_control()?;
        self.input = selected.preview.clone();
        self.editing_pending_control = Some(PendingControlEditorState {
            id: selected.id.clone(),
            kind: selected.kind,
        });
        self.pending_control_picker = None;
        Some(selected)
    }

    pub(crate) fn clear_pending_control_edit(&mut self) {
        self.editing_pending_control = None;
    }

    pub(crate) fn prime_history_rollback(&mut self) {
        self.main_pane = MainPaneMode::Transcript;
        self.pending_control_picker = None;
        self.statusline_picker = None;
        self.thinking_effort_picker = None;
        self.history_rollback = Some(HistoryRollbackState::Primed);
    }

    pub(crate) fn open_history_rollback_overlay(
        &mut self,
        candidates: Vec<HistoryRollbackCandidate>,
    ) -> bool {
        if candidates.is_empty() {
            return false;
        }
        self.main_pane = MainPaneMode::Transcript;
        self.pending_control_picker = None;
        self.statusline_picker = None;
        self.thinking_effort_picker = None;
        self.history_rollback = Some(HistoryRollbackState::Selecting(
            HistoryRollbackOverlayState {
                selected: candidates.len().saturating_sub(1),
                candidates,
            },
        ));
        true
    }

    pub(crate) fn clear_history_rollback(&mut self) {
        self.history_rollback = None;
    }

    pub(crate) fn history_rollback_is_primed(&self) -> bool {
        matches!(self.history_rollback, Some(HistoryRollbackState::Primed))
    }

    pub(crate) fn history_rollback_overlay(&self) -> Option<&HistoryRollbackOverlayState> {
        match self.history_rollback.as_ref() {
            Some(HistoryRollbackState::Selecting(overlay)) => Some(overlay),
            _ => None,
        }
    }

    pub(crate) fn move_history_rollback_selection(&mut self, backwards: bool) -> bool {
        let Some(HistoryRollbackState::Selecting(overlay)) = self.history_rollback.as_mut() else {
            return false;
        };
        let total = overlay.candidates.len();
        if total == 0 {
            return false;
        }
        overlay.selected = if backwards {
            overlay.selected.checked_sub(1).unwrap_or(total - 1)
        } else {
            (overlay.selected + 1) % total
        };
        true
    }

    pub(crate) fn jump_history_rollback_selection(&mut self, oldest: bool) -> bool {
        let Some(HistoryRollbackState::Selecting(overlay)) = self.history_rollback.as_mut() else {
            return false;
        };
        if overlay.candidates.is_empty() {
            return false;
        }
        overlay.selected = if oldest {
            0
        } else {
            overlay.candidates.len().saturating_sub(1)
        };
        true
    }

    pub(crate) fn selected_history_rollback_candidate(&self) -> Option<&HistoryRollbackCandidate> {
        let overlay = self.history_rollback_overlay()?;
        overlay.candidates.get(overlay.selected)
    }

    pub(crate) fn sync_pending_controls(&mut self, controls: Vec<PendingControlSummary>) {
        self.pending_controls = controls;
        if let Some(picker) = self.pending_control_picker.as_mut() {
            picker.selected = picker
                .selected
                .min(self.pending_controls.len().saturating_sub(1));
            if self.pending_controls.is_empty() {
                self.pending_control_picker = None;
            }
        }
        if let Some(editor) = self.editing_pending_control.as_ref()
            && !self
                .pending_controls
                .iter()
                .any(|control| control.id == editor.id)
        {
            self.editing_pending_control = None;
        }
    }

    pub(crate) fn move_statusline_picker(&mut self, backwards: bool) -> bool {
        let Some(picker) = self.statusline_picker.as_mut() else {
            return false;
        };
        let total = status_line_fields().len();
        if total == 0 {
            return false;
        }
        picker.selected = if backwards {
            picker.selected.checked_sub(1).unwrap_or(total - 1)
        } else {
            (picker.selected + 1) % total
        };
        true
    }

    pub(crate) fn selected_statusline_field(&self) -> Option<StatusLineField> {
        let picker = self.statusline_picker.as_ref()?;
        status_line_fields()
            .get(picker.selected)
            .map(|spec| spec.field)
    }

    pub(crate) fn toggle_selected_statusline_field(&mut self) -> Option<(StatusLineField, bool)> {
        let field = self.selected_statusline_field()?;
        let enabled = self.session.statusline.toggle(field);
        Some((field, enabled))
    }

    pub(crate) fn move_thinking_effort_picker(&mut self, backwards: bool) -> bool {
        let Some(picker) = self.thinking_effort_picker.as_mut() else {
            return false;
        };
        let total = self.session.supported_model_reasoning_efforts.len();
        if total == 0 {
            return false;
        }
        picker.selected = if backwards {
            picker.selected.checked_sub(1).unwrap_or(total - 1)
        } else {
            (picker.selected + 1) % total
        };
        true
    }

    pub(crate) fn selected_thinking_effort(&self) -> Option<String> {
        let picker = self.thinking_effort_picker.as_ref()?;
        self.session
            .supported_model_reasoning_efforts
            .get(picker.selected)
            .cloned()
    }

    pub(crate) fn push_activity(&mut self, line: impl Into<String>) {
        self.activity.push(line.into());
        self.activity_scroll = u16::MAX;
        if self.activity.len() > 128 {
            let overflow = self.activity.len() - 128;
            self.activity.drain(0..overflow);
        }
    }

    pub(crate) fn push_transcript(&mut self, entry: impl Into<TranscriptEntry>) {
        self.transcript.push(entry.into());
        self.mark_transcript_follow();
    }

    pub(crate) fn replace_transcript(
        &mut self,
        index: usize,
        entry: impl Into<TranscriptEntry>,
    ) -> bool {
        let Some(slot) = self.transcript.get_mut(index) else {
            return false;
        };
        *slot = entry.into();
        self.mark_transcript_follow();
        true
    }

    pub(crate) fn append_transcript_text(&mut self, index: usize, delta: &str) -> bool {
        let Some(entry) = self.transcript.get_mut(index) else {
            return false;
        };
        let appended = entry.append_text(delta);
        if appended {
            self.mark_transcript_follow();
        }
        appended
    }

    fn mark_transcript_follow(&mut self) {
        if self.follow_transcript {
            self.transcript_scroll = u16::MAX;
        }
    }

    pub(crate) fn reset_command_completion(&mut self) {
        self.command_completion_index = 0;
    }

    pub(crate) fn scroll_focused(&mut self, delta: i16) {
        match self.main_pane {
            MainPaneMode::Transcript => {
                // Manual transcript scrolling detaches the viewport from live
                // follow mode until the operator explicitly jumps back to end.
                self.follow_transcript = false;
                bump_scroll(&mut self.transcript_scroll, delta);
            }
            MainPaneMode::View => bump_scroll(&mut self.inspector_scroll, delta),
        }
    }

    pub(crate) fn scroll_focused_page(
        &mut self,
        viewport_height: u16,
        half_page: bool,
        backwards: bool,
    ) {
        let amount = page_scroll_amount(viewport_height, half_page);
        let delta = if backwards { -amount } else { amount };
        self.scroll_focused(delta);
    }

    pub(crate) fn scroll_focused_home(&mut self) {
        match self.main_pane {
            MainPaneMode::Transcript => {
                self.follow_transcript = false;
                self.transcript_scroll = 0;
            }
            MainPaneMode::View => self.inspector_scroll = 0,
        }
    }

    pub(crate) fn scroll_focused_end(&mut self) {
        match self.main_pane {
            MainPaneMode::Transcript => {
                self.follow_transcript = true;
                self.transcript_scroll = u16::MAX;
            }
            MainPaneMode::View => self.inspector_scroll = u16::MAX,
        }
    }
}

fn bump_scroll(value: &mut u16, delta: i16) {
    if delta >= 0 {
        *value = value.saturating_add(delta as u16);
    } else {
        *value = value.saturating_sub(delta.unsigned_abs());
    }
}

fn page_scroll_amount(viewport_height: u16, half_page: bool) -> i16 {
    let page = viewport_height.saturating_sub(2).max(1);
    let amount = if half_page { (page / 2).max(1) } else { page };
    amount.min(i16::MAX as u16) as i16
}

#[derive(Clone, Default)]
pub(crate) struct SharedUiState(Arc<RwLock<TuiState>>);

impl SharedUiState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn snapshot(&self) -> TuiState {
        self.0.read().unwrap().clone()
    }

    pub(crate) fn replace(&self, state: TuiState) {
        *self.0.write().unwrap() = state;
    }

    pub(crate) fn mutate<F>(&self, f: F)
    where
        F: FnOnce(&mut TuiState),
    {
        f(&mut self.0.write().unwrap());
    }

    pub(crate) fn take_input(&self) -> String {
        let mut state = self.0.write().unwrap();
        state.command_completion_index = 0;
        std::mem::take(&mut state.input)
    }
}

pub(crate) fn preview_text(value: &str, max_chars: usize) -> String {
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

pub(crate) fn git_snapshot(
    workspace_root: &Path,
    host_process_surfaces_allowed: bool,
) -> GitSnapshot {
    // The TUI git snapshot is a convenience-only host subprocess. When the
    // operator continues without sandbox enforcement, keep the UI on the same
    // fail-closed boundary as the runtime tool and hook surfaces.
    if !host_process_surfaces_allowed {
        return GitSnapshot::default();
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .arg("status")
        .arg("--short")
        .arg("--branch")
        .output();
    let Ok(output) = output else {
        return GitSnapshot::default();
    };
    if !output.status.success() {
        return GitSnapshot::default();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let branch = lines
        .next()
        .map(|line| line.trim_start_matches("## ").to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let repo_name = git_repo_name(workspace_root).unwrap_or_default();
    let mut staged = 0;
    let mut modified = 0;
    let mut untracked = 0;
    for line in lines {
        if line.starts_with("??") {
            untracked += 1;
            continue;
        }
        let bytes = line.as_bytes();
        if bytes.first().copied().unwrap_or(b' ') != b' ' {
            staged += 1;
        }
        if bytes.get(1).copied().unwrap_or(b' ') != b' ' {
            modified += 1;
        }
    }
    GitSnapshot {
        available: true,
        repo_name,
        branch,
        staged,
        modified,
        untracked,
    }
}

fn git_repo_name(workspace_root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace_root)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Path::new(&root)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::{
        HistoryRollbackCandidate, MainPaneMode, TuiState, git_snapshot, page_scroll_amount,
    };
    use agent::types::MessageId;
    use tempfile::tempdir;

    #[test]
    fn git_snapshot_skips_host_process_when_disabled() {
        let dir = tempdir().unwrap();
        let snapshot = git_snapshot(dir.path(), false);

        assert!(!snapshot.available);
        assert!(snapshot.repo_name.is_empty());
        assert!(snapshot.branch.is_empty());
    }

    #[test]
    fn transcript_push_keeps_manual_scroll_position_until_follow_is_restored() {
        let mut state = TuiState {
            main_pane: MainPaneMode::Transcript,
            follow_transcript: true,
            ..TuiState::default()
        };

        state.push_transcript("first");
        assert_eq!(state.transcript_scroll, u16::MAX);

        state.scroll_focused(-2);
        assert!(!state.follow_transcript);

        state.push_transcript("second");
        assert_eq!(state.transcript_scroll, u16::MAX.saturating_sub(2));

        state.scroll_focused_end();
        assert!(state.follow_transcript);

        state.push_transcript("third");
        assert_eq!(state.transcript_scroll, u16::MAX);
    }

    #[test]
    fn transcript_home_disables_follow_until_end_is_requested() {
        let mut state = TuiState {
            main_pane: MainPaneMode::Transcript,
            follow_transcript: true,
            ..TuiState::default()
        };

        state.scroll_focused_home();
        assert_eq!(state.transcript_scroll, 0);
        assert!(!state.follow_transcript);

        state.scroll_focused_end();
        assert_eq!(state.transcript_scroll, u16::MAX);
        assert!(state.follow_transcript);
    }

    #[test]
    fn page_scroll_amount_keeps_overlap_and_supports_half_pages() {
        assert_eq!(page_scroll_amount(20, false), 18);
        assert_eq!(page_scroll_amount(20, true), 9);
        assert_eq!(page_scroll_amount(1, false), 1);
        assert_eq!(page_scroll_amount(2, true), 1);
    }

    #[test]
    fn transcript_page_scroll_uses_viewport_height() {
        let mut state = TuiState {
            main_pane: MainPaneMode::Transcript,
            follow_transcript: true,
            transcript_scroll: 40,
            ..TuiState::default()
        };

        state.scroll_focused_page(20, true, true);
        assert_eq!(state.transcript_scroll, 31);
        assert!(!state.follow_transcript);

        state.scroll_focused_page(20, false, false);
        assert_eq!(state.transcript_scroll, 49);
    }

    #[test]
    fn history_rollback_overlay_opens_on_latest_candidate_and_wraps_navigation() {
        let mut state = TuiState::default();
        let candidates = vec![
            HistoryRollbackCandidate {
                message_id: MessageId::from("msg-1"),
                prompt: "first".to_string(),
                turn_preview_lines: vec!["› first".into()],
                removed_turn_count: 2,
                removed_message_count: 4,
            },
            HistoryRollbackCandidate {
                message_id: MessageId::from("msg-2"),
                prompt: "second".to_string(),
                turn_preview_lines: vec!["› second".into()],
                removed_turn_count: 1,
                removed_message_count: 2,
            },
        ];

        assert!(state.open_history_rollback_overlay(candidates));
        assert_eq!(
            state
                .selected_history_rollback_candidate()
                .map(|candidate| candidate.prompt.as_str()),
            Some("second")
        );

        assert!(state.move_history_rollback_selection(true));
        assert_eq!(
            state
                .selected_history_rollback_candidate()
                .map(|candidate| candidate.prompt.as_str()),
            Some("first")
        );

        assert!(state.move_history_rollback_selection(true));
        assert_eq!(
            state
                .selected_history_rollback_candidate()
                .map(|candidate| candidate.prompt.as_str()),
            Some("second")
        );
    }
}
