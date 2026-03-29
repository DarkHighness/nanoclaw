use crate::backend::StartupDiagnosticsSnapshot;
use agent::types::TokenLedgerSnapshot;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, RwLock};
use std::time::Instant;

#[derive(Clone, Debug, Default)]
pub(crate) struct GitSnapshot {
    pub(crate) available: bool,
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
pub(crate) enum MainPaneMode {
    #[default]
    Transcript,
    View,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TuiState {
    pub(crate) session: SessionSummary,
    pub(crate) main_pane: MainPaneMode,
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
    pub(crate) turn_started_at: Option<Instant>,
}

impl TuiState {
    pub(crate) fn show_main_view(&mut self, title: impl Into<String>, lines: Vec<String>) {
        self.main_pane = MainPaneMode::View;
        self.inspector_title = title.into();
        self.inspector = lines;
        self.inspector_scroll = 0;
    }

    pub(crate) fn show_transcript_pane(&mut self) {
        self.main_pane = MainPaneMode::Transcript;
    }

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

    pub(crate) fn scroll_focused(&mut self, delta: i16) {
        match self.main_pane {
            MainPaneMode::Transcript => bump_scroll(&mut self.transcript_scroll, delta),
            MainPaneMode::View => bump_scroll(&mut self.inspector_scroll, delta),
        }
    }

    pub(crate) fn scroll_focused_home(&mut self) {
        match self.main_pane {
            MainPaneMode::Transcript => self.transcript_scroll = 0,
            MainPaneMode::View => self.inspector_scroll = 0,
        }
    }

    pub(crate) fn scroll_focused_end(&mut self) {
        match self.main_pane {
            MainPaneMode::Transcript => self.transcript_scroll = u16::MAX,
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
