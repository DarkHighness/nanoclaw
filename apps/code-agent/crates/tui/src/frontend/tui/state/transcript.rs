use super::*;

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
    pub(crate) warnings: Vec<String>,
    pub(crate) items: Vec<PlanEntry>,
}

impl TranscriptPlanEntry {
    pub(crate) fn new(
        explanation: Option<String>,
        warnings: Vec<String>,
        items: Vec<PlanEntry>,
    ) -> Self {
        Self {
            headline: "Updated Plan".to_string(),
            explanation: explanation
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            warnings: warnings
                .into_iter()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect(),
            items,
        }
    }

    pub(crate) fn serialized_lines(&self) -> Vec<String> {
        let mut lines = vec![self.headline.clone()];
        if let Some(explanation) = &self.explanation {
            lines.push(format!("  └ {explanation}"));
        }
        lines.extend(
            self.warnings
                .iter()
                .map(|warning| format!("  └ warning {warning}")),
        );
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TranscriptExecutionEntry {
    pub(crate) headline: String,
    pub(crate) state: Option<ExecutionEntry>,
}

impl TranscriptExecutionEntry {
    pub(crate) fn new(headline: impl Into<String>, state: Option<ExecutionEntry>) -> Self {
        Self {
            headline: headline.into(),
            state,
        }
    }

    pub(crate) fn serialized_lines(&self) -> Vec<String> {
        let mut lines = vec![self.headline.clone()];
        let Some(state) = &self.state else {
            lines.push("  └ (cleared)".to_string());
            return lines;
        };
        lines.push(format!(
            "  └ [{}] {}",
            execution_status_marker(&state.status),
            state.summary
        ));
        if !state.scope_label.is_empty() {
            lines.push(format!("  └ scope {}", state.scope_label));
        }
        if let Some(next_action) = state.next_action.as_deref() {
            lines.push(format!("  └ next {next_action}"));
        }
        if let Some(verification) = state.verification.as_deref() {
            lines.push(format!("  └ verify {verification}"));
        }
        if let Some(blocker) = state.blocker.as_deref() {
            lines.push(format!("  └ blocker {blocker}"));
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

fn execution_status_marker(status: &str) -> &'static str {
    match status {
        "completed" => "x",
        "blocked" => "!",
        "verifying" => "~",
        _ => ">",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TranscriptEntry {
    UserPrompt(String),
    AssistantMessage(String),
    Tool(TranscriptToolEntry),
    Plan(TranscriptPlanEntry),
    Execution(TranscriptExecutionEntry),
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

    pub(crate) fn plan_update(
        explanation: Option<String>,
        warnings: Vec<String>,
        items: Vec<PlanEntry>,
    ) -> Self {
        Self::Plan(TranscriptPlanEntry::new(explanation, warnings, items))
    }

    pub(crate) fn execution_update(
        headline: impl Into<String>,
        state: Option<ExecutionEntry>,
    ) -> Self {
        Self::Execution(TranscriptExecutionEntry::new(headline, state))
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

    pub(crate) fn warning_summary_details(
        headline: impl Into<String>,
        detail_lines: Vec<TranscriptShellDetail>,
    ) -> Self {
        Self::WarningSummary(TranscriptShellEntry::new(headline, detail_lines))
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
            | Self::Execution(_)
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
            Self::Execution(entry) => format!("• {}", entry.serialized_body()),
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
            Self::Execution(_) => "•",
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
            Self::Execution(entry) => entry.headline.as_str(),
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
            Self::UserPrompt(_)
            | Self::AssistantMessage(_)
            | Self::Tool(_)
            | Self::Plan(_)
            | Self::Execution(_) => None,
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
            | Self::Execution(_)
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
            | Self::Execution(_)
            | Self::ShellSummary(_)
            | Self::SuccessSummary(_)
            | Self::ErrorSummary(_)
            | Self::WarningSummary(_) => None,
        }
    }

    pub(crate) fn execution_entry(&self) -> Option<&TranscriptExecutionEntry> {
        match self {
            Self::Execution(entry) => Some(entry),
            Self::UserPrompt(_)
            | Self::AssistantMessage(_)
            | Self::Tool(_)
            | Self::Plan(_)
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
