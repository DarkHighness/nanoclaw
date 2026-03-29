use crate::backend::StartupDiagnosticsSnapshot;
use agent::types::TokenLedgerSnapshot;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, RwLock};

#[derive(Clone, Debug, Default)]
pub(crate) struct GitSnapshot {
    pub(crate) available: bool,
    pub(crate) branch: String,
    pub(crate) staged: usize,
    pub(crate) modified: usize,
    pub(crate) untracked: usize,
}

impl GitSnapshot {
    pub(crate) fn branch_label(&self) -> String {
        if self.available {
            format!("git: {}", self.branch)
        } else {
            "git: unavailable".to_string()
        }
    }

    pub(crate) fn dirty_label(&self) -> String {
        if !self.available {
            return "repo unavailable".to_string();
        }
        format!(
            "staged {}  modified {}  untracked {}",
            self.staged, self.modified, self.untracked
        )
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.staged > 0 || self.modified > 0 || self.untracked > 0
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SessionSummary {
    pub(crate) workspace_name: String,
    pub(crate) active_session_ref: String,
    pub(crate) root_agent_session_id: String,
    pub(crate) provider_label: String,
    pub(crate) model: String,
    pub(crate) summary_model: String,
    pub(crate) memory_model: String,
    pub(crate) workspace_root: PathBuf,
    pub(crate) git: GitSnapshot,
    pub(crate) tool_names: Vec<String>,
    pub(crate) skill_names: Vec<String>,
    pub(crate) store_label: String,
    pub(crate) store_warning: Option<String>,
    pub(crate) stored_session_count: usize,
    pub(crate) sandbox_summary: String,
    pub(crate) startup_diagnostics: StartupDiagnosticsSnapshot,
    pub(crate) queued_commands: usize,
    pub(crate) token_ledger: TokenLedgerSnapshot,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PaneFocus {
    #[default]
    Conversation,
    Inspector,
    Activity,
}

impl PaneFocus {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Conversation => "Conversation",
            Self::Inspector => "Focus",
            Self::Activity => "Activity Feed",
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::Conversation => Self::Inspector,
            Self::Inspector => Self::Activity,
            Self::Activity => Self::Conversation,
        }
    }

    pub(crate) fn previous(self) -> Self {
        match self {
            Self::Conversation => Self::Activity,
            Self::Inspector => Self::Conversation,
            Self::Activity => Self::Inspector,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TuiState {
    pub(crate) session: SessionSummary,
    pub(crate) focus: PaneFocus,
    pub(crate) input: String,
    pub(crate) transcript: Vec<String>,
    pub(crate) transcript_scroll: u16,
    pub(crate) inspector_title: String,
    pub(crate) inspector: Vec<String>,
    pub(crate) inspector_scroll: u16,
    pub(crate) activity: Vec<String>,
    pub(crate) activity_scroll: u16,
    pub(crate) status: String,
    pub(crate) turn_running: bool,
}

impl TuiState {
    pub(crate) fn push_activity(&mut self, line: impl Into<String>) {
        self.activity.push(line.into());
        self.activity_scroll = u16::MAX;
        if self.activity.len() > 128 {
            let overflow = self.activity.len() - 128;
            self.activity.drain(0..overflow);
        }
    }

    pub(crate) fn push_transcript(&mut self, line: impl Into<String>) {
        self.transcript.push(line.into());
        self.transcript_scroll = u16::MAX;
    }

    pub(crate) fn cycle_focus_forward(&mut self) {
        self.focus = self.focus.next();
    }

    pub(crate) fn cycle_focus_backward(&mut self) {
        self.focus = self.focus.previous();
    }

    pub(crate) fn scroll_focused(&mut self, delta: i16) {
        match self.focus {
            PaneFocus::Conversation => bump_scroll(&mut self.transcript_scroll, delta),
            PaneFocus::Inspector => bump_scroll(&mut self.inspector_scroll, delta),
            PaneFocus::Activity => bump_scroll(&mut self.activity_scroll, delta),
        }
    }

    pub(crate) fn scroll_focused_home(&mut self) {
        match self.focus {
            PaneFocus::Conversation => self.transcript_scroll = 0,
            PaneFocus::Inspector => self.inspector_scroll = 0,
            PaneFocus::Activity => self.activity_scroll = 0,
        }
    }

    pub(crate) fn scroll_focused_end(&mut self) {
        match self.focus {
            PaneFocus::Conversation => self.transcript_scroll = u16::MAX,
            PaneFocus::Inspector => self.inspector_scroll = u16::MAX,
            PaneFocus::Activity => self.activity_scroll = u16::MAX,
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
        std::mem::take(&mut self.0.write().unwrap().input)
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

pub(crate) fn git_snapshot(workspace_root: &Path) -> GitSnapshot {
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
        branch,
        staged,
        modified,
        untracked,
    }
}
