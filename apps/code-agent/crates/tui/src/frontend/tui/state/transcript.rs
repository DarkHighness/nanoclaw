use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TranscriptSerializedPrefix {
    UserPrompt,
    Bullet,
    Success,
    Error,
    Warning,
}

impl TranscriptSerializedPrefix {
    pub(crate) fn marker(self) -> &'static str {
        match self {
            Self::UserPrompt => "› ",
            Self::Bullet => "• ",
            Self::Success => "✔ ",
            Self::Error => "✗ ",
            Self::Warning => "⚠ ",
        }
    }

    pub(crate) fn parse(line: &str) -> Option<(Self, &str)> {
        [
            Self::UserPrompt,
            Self::Success,
            Self::Error,
            Self::Warning,
            Self::Bullet,
        ]
        .into_iter()
        .find_map(|prefix| {
            line.strip_prefix(prefix.marker())
                .map(|body| (prefix, body))
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TranscriptDetailPrefix {
    Branch,
    Continuation,
}

impl TranscriptDetailPrefix {
    pub(crate) fn marker(self) -> &'static str {
        match self {
            Self::Branch => "  └ ",
            Self::Continuation => "    ",
        }
    }

    pub(crate) fn parse(line: &str) -> Option<(Self, &str)> {
        [Self::Branch, Self::Continuation]
            .into_iter()
            .find_map(|prefix| {
                line.strip_prefix(prefix.marker())
                    .map(|body| (prefix, body))
            })
    }
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
        if let Some((prefix, detail)) = TranscriptDetailPrefix::parse(raw) {
            return Some(Self::Raw {
                text: detail.to_string(),
                continuation: prefix == TranscriptDetailPrefix::Continuation,
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
                let prefix = if *continuation {
                    TranscriptDetailPrefix::Continuation
                } else {
                    TranscriptDetailPrefix::Branch
                };
                vec![format!("{}{text}", prefix.marker())]
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TranscriptShellEntry {
    pub(crate) headline: String,
    pub(crate) status: Option<TranscriptShellStatus>,
    pub(crate) detail_lines: Vec<TranscriptShellDetail>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TranscriptShellStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl TranscriptShellEntry {
    pub(crate) fn new(
        headline: impl Into<String>,
        detail_lines: Vec<TranscriptShellDetail>,
    ) -> Self {
        Self::new_with_status(headline, None, detail_lines)
    }

    pub(crate) fn new_with_status(
        headline: impl Into<String>,
        status: Option<TranscriptShellStatus>,
        detail_lines: Vec<TranscriptShellDetail>,
    ) -> Self {
        Self {
            headline: headline.into(),
            status,
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
            let Some((prefix, detail)) = TranscriptDetailPrefix::parse(raw_line) else {
                if let Some(detail_line) = TranscriptShellDetail::from_prefixed(raw_line) {
                    detail_lines.push(detail_line);
                }
                index += 1;
                continue;
            };
            if prefix != TranscriptDetailPrefix::Branch {
                if let Some(detail_line) = TranscriptShellDetail::from_prefixed(raw_line) {
                    detail_lines.push(detail_line);
                }
                index += 1;
                continue;
            }

            let mut block_lines = Vec::new();
            let mut next = index + 1;
            while let Some(continuation) = remaining.get(next).copied() {
                let Some((prefix, continuation)) = TranscriptDetailPrefix::parse(continuation)
                else {
                    break;
                };
                if prefix != TranscriptDetailPrefix::Continuation {
                    break;
                }
                block_lines.push(continuation.to_string());
                next += 1;
            }

            detail_lines.push(classify_shell_detail(detail, block_lines));
            index = next;
        }

        Self::new_with_status(headline, None, detail_lines)
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

        Self::new_with_status(self.headline.clone(), self.status, detail_lines)
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
        let prefix = if index == 0 {
            TranscriptDetailPrefix::Branch
        } else {
            TranscriptDetailPrefix::Continuation
        };
        rendered.push(format!("{}{line}", prefix.marker()));
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
    fn marker(self) -> &'static str {
        let _ = self;
        "•"
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TranscriptToolHeadlineSubjectKind {
    None,
    Command,
    ToolName,
}

fn default_tool_completion(status: TranscriptToolStatus) -> ToolCompletionState {
    match status {
        TranscriptToolStatus::Requested
        | TranscriptToolStatus::WaitingApproval
        | TranscriptToolStatus::Running => ToolCompletionState::Neutral,
        TranscriptToolStatus::Denied
        | TranscriptToolStatus::Failed
        | TranscriptToolStatus::Cancelled => ToolCompletionState::Failure,
        TranscriptToolStatus::Approved | TranscriptToolStatus::Finished => {
            ToolCompletionState::Success
        }
    }
}

fn tool_headline_text(
    status: TranscriptToolStatus,
    tool_name: &str,
    detail_lines: &[ToolDetail],
) -> String {
    let prefix = tool_headline_prefix(status, tool_name, detail_lines);
    match tool_headline_subject_kind(tool_name, detail_lines) {
        TranscriptToolHeadlineSubjectKind::None => prefix.to_string(),
        TranscriptToolHeadlineSubjectKind::Command => detail_lines
            .iter()
            .find_map(|detail| match detail {
                ToolDetail::Command(command) => Some(format!("{prefix} {}", command.raw)),
                _ => None,
            })
            .unwrap_or_else(|| prefix.to_string()),
        TranscriptToolHeadlineSubjectKind::ToolName => format!("{prefix} {tool_name}"),
    }
}

fn tool_headline_prefix(
    status: TranscriptToolStatus,
    tool_name: &str,
    detail_lines: &[ToolDetail],
) -> &'static str {
    if let Some(command) = detail_lines.iter().find_map(|detail| match detail {
        ToolDetail::Command(command) => Some(command),
        _ => None,
    }) {
        return match command.intent {
            ToolCommandIntent::Execute => command_headline_text(status),
            ToolCommandIntent::Explore => exploration_headline_text(status),
        };
    }

    match ToolRenderKind::classify(tool_name) {
        ToolRenderKind::CronCreate => {
            lifecycle_headline_text(status, "Scheduling automation", "Scheduled automation")
        }
        ToolRenderKind::CronList => {
            lifecycle_headline_text(status, "Listing automations", "Listed automations")
        }
        ToolRenderKind::CronDelete => {
            lifecycle_headline_text(status, "Cancelling automation", "Cancelled automation")
        }
        ToolRenderKind::NotebookEdit => {
            lifecycle_headline_text(status, "Editing notebook", "Updated notebook")
        }
        ToolRenderKind::NotebookRead => {
            lifecycle_headline_text(status, "Reading notebook", "Read notebook")
        }
        ToolRenderKind::CodeSearch => {
            lifecycle_headline_text(status, "Searching code", "Searched code")
        }
        ToolRenderKind::CodeDiagnostics => {
            lifecycle_headline_text(status, "Inspecting diagnostics", "Inspected diagnostics")
        }
        ToolRenderKind::BrowserOpen => {
            lifecycle_headline_text(status, "Opening browser", "Opened browser")
        }
        ToolRenderKind::BrowserSnapshot => {
            lifecycle_headline_text(status, "Inspecting browser", "Inspected browser")
        }
        ToolRenderKind::BrowserClick => {
            lifecycle_headline_text(status, "Clicking browser", "Clicked browser")
        }
        ToolRenderKind::SendInput => {
            lifecycle_headline_text(status, "Sending follow-up", "Sent follow-up")
        }
        ToolRenderKind::SpawnAgent => {
            lifecycle_headline_text(status, "Spawning agent", "Spawned agent")
        }
        ToolRenderKind::WaitAgent => {
            lifecycle_headline_text(status, "Waiting on agents", "Waited on agents")
        }
        ToolRenderKind::ResumeAgent => {
            lifecycle_headline_text(status, "Resuming agent", "Resumed agent")
        }
        ToolRenderKind::CloseAgent => {
            lifecycle_headline_text(status, "Closing agent", "Closed agent")
        }
        ToolRenderKind::FileMutation => {
            lifecycle_headline_text(status, "Editing files", "Updated files")
        }
        ToolRenderKind::MonitorStart => {
            lifecycle_headline_text(status, "Starting monitor", "Started monitor")
        }
        ToolRenderKind::MonitorList => {
            lifecycle_headline_text(status, "Listing monitors", "Listed monitors")
        }
        ToolRenderKind::MonitorStop => {
            lifecycle_headline_text(status, "Stopping monitor", "Stopped monitor")
        }
        ToolRenderKind::WorktreeEnter => {
            lifecycle_headline_text(status, "Entering worktree", "Entered worktree")
        }
        ToolRenderKind::WorktreeList => {
            lifecycle_headline_text(status, "Listing worktrees", "Listed worktrees")
        }
        ToolRenderKind::WorktreeExit => {
            lifecycle_headline_text(status, "Exiting worktree", "Exited worktree")
        }
        ToolRenderKind::ExecCommand | ToolRenderKind::WriteStdin | ToolRenderKind::Generic => {
            generic_headline_text(status)
        }
    }
}

fn tool_headline_subject_kind(
    tool_name: &str,
    detail_lines: &[ToolDetail],
) -> TranscriptToolHeadlineSubjectKind {
    if let Some(command) = detail_lines.iter().find_map(|detail| match detail {
        ToolDetail::Command(command) => Some(command),
        _ => None,
    }) {
        return match command.intent {
            ToolCommandIntent::Execute => TranscriptToolHeadlineSubjectKind::Command,
            ToolCommandIntent::Explore => TranscriptToolHeadlineSubjectKind::None,
        };
    }

    match ToolRenderKind::classify(tool_name) {
        ToolRenderKind::ExecCommand | ToolRenderKind::WriteStdin | ToolRenderKind::Generic => {
            TranscriptToolHeadlineSubjectKind::ToolName
        }
        ToolRenderKind::CronCreate
        | ToolRenderKind::CronList
        | ToolRenderKind::CronDelete
        | ToolRenderKind::NotebookEdit
        | ToolRenderKind::NotebookRead
        | ToolRenderKind::CodeSearch
        | ToolRenderKind::CodeDiagnostics
        | ToolRenderKind::BrowserOpen
        | ToolRenderKind::BrowserSnapshot
        | ToolRenderKind::BrowserClick
        | ToolRenderKind::MonitorStart
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
        | ToolRenderKind::FileMutation => TranscriptToolHeadlineSubjectKind::None,
    }
}

fn lifecycle_headline_text(
    status: TranscriptToolStatus,
    running: &'static str,
    finished: &'static str,
) -> &'static str {
    match status {
        TranscriptToolStatus::Requested | TranscriptToolStatus::Running => running,
        TranscriptToolStatus::WaitingApproval => "Awaiting approval",
        TranscriptToolStatus::Approved => "Approved",
        TranscriptToolStatus::Finished => finished,
        TranscriptToolStatus::Denied => "Denied",
        TranscriptToolStatus::Failed => "Failed",
        TranscriptToolStatus::Cancelled => "Cancelled",
    }
}

fn generic_headline_text(status: TranscriptToolStatus) -> &'static str {
    match status {
        TranscriptToolStatus::Requested | TranscriptToolStatus::Running => "Calling",
        TranscriptToolStatus::WaitingApproval => "Awaiting approval",
        TranscriptToolStatus::Approved => "Approved",
        TranscriptToolStatus::Finished => "Called",
        TranscriptToolStatus::Denied => "Denied",
        TranscriptToolStatus::Failed => "Failed",
        TranscriptToolStatus::Cancelled => "Cancelled",
    }
}

fn command_headline_text(status: TranscriptToolStatus) -> &'static str {
    match status {
        TranscriptToolStatus::Requested => "Will run",
        TranscriptToolStatus::WaitingApproval => "Awaiting approval to run",
        TranscriptToolStatus::Approved => "Approved command",
        TranscriptToolStatus::Running => "Running",
        TranscriptToolStatus::Finished => "Ran",
        TranscriptToolStatus::Denied => "Denied command",
        TranscriptToolStatus::Failed => "Command failed",
        TranscriptToolStatus::Cancelled => "Cancelled command",
    }
}

fn exploration_headline_text(status: TranscriptToolStatus) -> &'static str {
    match status {
        TranscriptToolStatus::Requested => "Will explore",
        TranscriptToolStatus::WaitingApproval => "Awaiting approval to explore",
        TranscriptToolStatus::Approved => "Approved exploration",
        TranscriptToolStatus::Running => "Exploring",
        TranscriptToolStatus::Finished => "Explored",
        TranscriptToolStatus::Denied => "Denied exploration",
        TranscriptToolStatus::Failed => "Exploration failed",
        TranscriptToolStatus::Cancelled => "Cancelled exploration",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TranscriptToolEntry {
    pub(crate) status: TranscriptToolStatus,
    pub(crate) completion: ToolCompletionState,
    pub(crate) tool_name: String,
    pub(crate) headline: String,
    pub(crate) detail_lines: Vec<ToolDetail>,
    pub(crate) review: Option<ToolReview>,
}

impl TranscriptToolEntry {
    pub(crate) fn new(
        status: TranscriptToolStatus,
        tool_name: impl Into<String>,
        detail_lines: Vec<ToolDetail>,
    ) -> Self {
        Self::new_with_review_and_completion(
            status,
            tool_name,
            detail_lines,
            None,
            default_tool_completion(status),
        )
    }

    #[cfg(test)]
    pub(crate) fn new_with_review(
        status: TranscriptToolStatus,
        tool_name: impl Into<String>,
        detail_lines: Vec<ToolDetail>,
        review: Option<ToolReview>,
    ) -> Self {
        Self::new_with_review_and_completion(
            status,
            tool_name,
            detail_lines,
            review,
            default_tool_completion(status),
        )
    }

    pub(crate) fn new_with_review_and_completion(
        status: TranscriptToolStatus,
        tool_name: impl Into<String>,
        detail_lines: Vec<ToolDetail>,
        review: Option<ToolReview>,
        completion: ToolCompletionState,
    ) -> Self {
        let tool_name = tool_name.into();
        let headline = tool_headline_text(status, &tool_name, &detail_lines);
        Self {
            status,
            completion,
            tool_name,
            headline,
            detail_lines,
            review,
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
            completion: self.completion,
            tool_name: self.tool_name.clone(),
            headline: self.headline.clone(),
            detail_lines: preview_tool_details(&self.detail_lines, max_lines),
            review: self.review.clone(),
        }
    }

    pub(crate) fn headline_prefix(&self) -> &'static str {
        tool_headline_prefix(self.status, &self.tool_name, &self.detail_lines)
    }

    pub(crate) fn headline_subject_kind(&self) -> TranscriptToolHeadlineSubjectKind {
        tool_headline_subject_kind(&self.tool_name, &self.detail_lines)
    }

    fn is_finished_successful_exploration(&self) -> bool {
        self.status == TranscriptToolStatus::Finished
            && self.completion == ToolCompletionState::Success
            && self
                .primary_command()
                .is_some_and(|command| command.intent == ToolCommandIntent::Explore)
    }

    fn try_merge_finished_exploration(&mut self, next: &Self) -> bool {
        if self.tool_name != next.tool_name
            || !self.is_finished_successful_exploration()
            || !next.is_finished_successful_exploration()
        {
            return false;
        }

        let Some(self_index) = self.first_command_detail_index() else {
            return false;
        };
        let Some(next_command) = next.primary_command().cloned() else {
            return false;
        };

        let ToolDetail::Command(command) = &mut self.detail_lines[self_index] else {
            return false;
        };
        if !command.merge_exploration(&next_command) {
            return false;
        }

        self.headline = tool_headline_text(self.status, &self.tool_name, &self.detail_lines);
        true
    }

    fn first_command_detail_index(&self) -> Option<usize> {
        self.detail_lines
            .iter()
            .position(|detail| matches!(detail, ToolDetail::Command(_)))
    }

    fn primary_command(&self) -> Option<&crate::tool_render::ToolCommand> {
        self.detail_lines.iter().find_map(|detail| match detail {
            ToolDetail::Command(command) => Some(command),
            _ => None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TranscriptEntry {
    UserPrompt(String),
    AssistantMessage(String),
    Tool(TranscriptToolEntry),
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

    pub(crate) fn tool_with_completion(
        status: TranscriptToolStatus,
        tool_name: impl Into<String>,
        detail_lines: Vec<ToolDetail>,
        completion: ToolCompletionState,
    ) -> Self {
        Self::Tool(TranscriptToolEntry::new_with_review_and_completion(
            status,
            tool_name,
            detail_lines,
            None,
            completion,
        ))
    }

    #[cfg(test)]
    pub(crate) fn tool_with_review(
        status: TranscriptToolStatus,
        tool_name: impl Into<String>,
        detail_lines: Vec<ToolDetail>,
        review: Option<ToolReview>,
    ) -> Self {
        Self::Tool(TranscriptToolEntry::new_with_review(
            status,
            tool_name,
            detail_lines,
            review,
        ))
    }

    pub(crate) fn tool_with_review_and_completion(
        status: TranscriptToolStatus,
        tool_name: impl Into<String>,
        detail_lines: Vec<ToolDetail>,
        review: Option<ToolReview>,
        completion: ToolCompletionState,
    ) -> Self {
        Self::Tool(TranscriptToolEntry::new_with_review_and_completion(
            status,
            tool_name,
            detail_lines,
            review,
            completion,
        ))
    }

    pub(crate) fn shell_summary_details(
        headline: impl Into<String>,
        detail_lines: Vec<TranscriptShellDetail>,
    ) -> Self {
        Self::ShellSummary(TranscriptShellEntry::new(headline, detail_lines))
    }

    pub(crate) fn shell_summary_status_details(
        status: TranscriptShellStatus,
        headline: impl Into<String>,
        detail_lines: Vec<TranscriptShellDetail>,
    ) -> Self {
        Self::ShellSummary(TranscriptShellEntry::new_with_status(
            headline,
            Some(status),
            detail_lines,
        ))
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
            | Self::ShellSummary(_)
            | Self::SuccessSummary(_)
            | Self::ErrorSummary(_)
            | Self::WarningSummary(_) => false,
        }
    }

    pub(crate) fn try_merge(&mut self, next: &Self) -> bool {
        match (self, next) {
            (Self::Tool(current), Self::Tool(next)) => current.try_merge_finished_exploration(next),
            _ => false,
        }
    }

    pub(crate) fn serialized(&self) -> String {
        match self {
            Self::UserPrompt(text) => {
                format!(
                    "{}{}",
                    TranscriptSerializedPrefix::UserPrompt.marker(),
                    text
                )
            }
            Self::AssistantMessage(text) => {
                format!("{}{}", TranscriptSerializedPrefix::Bullet.marker(), text)
            }
            Self::Tool(entry) => format!("{} {}", entry.marker(), entry.serialized_body()),
            Self::ShellSummary(summary) => format!(
                "{}{}",
                TranscriptSerializedPrefix::Bullet.marker(),
                summary.serialized_body()
            ),
            Self::SuccessSummary(summary) => format!(
                "{}{}",
                TranscriptSerializedPrefix::Success.marker(),
                summary.serialized_body()
            ),
            Self::ErrorSummary(summary) => format!(
                "{}{}",
                TranscriptSerializedPrefix::Error.marker(),
                summary.serialized_body()
            ),
            Self::WarningSummary(summary) => format!(
                "{}{}",
                TranscriptSerializedPrefix::Warning.marker(),
                summary.serialized_body()
            ),
        }
    }

    pub(crate) fn marker(&self) -> &'static str {
        match self {
            Self::UserPrompt(_) => "›",
            Self::AssistantMessage(_) | Self::ShellSummary(_) => "•",
            Self::Tool(entry) => entry.marker(),
            Self::SuccessSummary(_) => "✔",
            Self::ErrorSummary(_) => "✗",
            Self::WarningSummary(_) => "⚠",
        }
    }

    pub(crate) fn body(&self) -> &str {
        match self {
            Self::UserPrompt(text) | Self::AssistantMessage(text) => text,
            Self::Tool(entry) => entry.headline.as_str(),
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
            Self::UserPrompt(_) | Self::AssistantMessage(_) | Self::Tool(_) => None,
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
        if let Some((prefix, body)) = TranscriptSerializedPrefix::parse(&value) {
            return match prefix {
                TranscriptSerializedPrefix::UserPrompt => Self::UserPrompt(body.to_string()),
                TranscriptSerializedPrefix::Success => {
                    Self::SuccessSummary(TranscriptShellEntry::from_body(body))
                }
                TranscriptSerializedPrefix::Error => {
                    Self::ErrorSummary(TranscriptShellEntry::from_body(body))
                }
                TranscriptSerializedPrefix::Warning => {
                    Self::WarningSummary(TranscriptShellEntry::from_body(body))
                }
                TranscriptSerializedPrefix::Bullet => {
                    if body.lines().any(|line| {
                        TranscriptDetailPrefix::parse(line)
                            .is_some_and(|(prefix, _)| prefix == TranscriptDetailPrefix::Branch)
                    }) {
                        Self::ShellSummary(TranscriptShellEntry::from_body(body))
                    } else {
                        Self::AssistantMessage(body.to_string())
                    }
                }
            };
        }
        Self::AssistantMessage(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InspectorAction {
    RunCommand(String),
    FillInput(String),
    LoadMcpPrompt {
        server_name: String,
        prompt_name: String,
    },
    LoadMcpResource {
        server_name: String,
        uri: String,
    },
    WaitLiveTask {
        task_or_agent_ref: String,
    },
    CancelLiveTask {
        task_or_agent_ref: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InspectorKeyAction {
    pub(crate) key_hint: String,
    pub(crate) label: String,
    pub(crate) action: InspectorAction,
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
        action: Option<InspectorAction>,
        alternate_action: Option<InspectorKeyAction>,
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
            action: None,
            alternate_action: None,
        }
    }

    pub(crate) fn actionable_collection(
        primary: impl Into<String>,
        secondary: Option<impl Into<String>>,
        action: InspectorAction,
    ) -> Self {
        Self::CollectionItem {
            primary: primary.into(),
            secondary: secondary.map(Into::into),
            action: Some(action),
            alternate_action: None,
        }
    }

    pub(crate) fn actionable_collection_with_alt(
        primary: impl Into<String>,
        secondary: Option<impl Into<String>>,
        action: InspectorAction,
        alternate_action: InspectorKeyAction,
    ) -> Self {
        Self::CollectionItem {
            primary: primary.into(),
            secondary: secondary.map(Into::into),
            action: Some(action),
            alternate_action: Some(alternate_action),
        }
    }
}
