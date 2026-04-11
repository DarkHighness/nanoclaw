mod approval;
mod commands;
mod composer;
mod history;
mod history_rollback;
mod input_history;
mod interaction_keys;
mod observer;
mod operator_support;
mod paste_burst;
mod render;
mod runtime_flow;
mod session_bridge;
mod session_shell;
mod slash_commands;
mod state;
mod terminal_shell;
mod tool_review;
mod tool_state;

use crate::backend::{CodeAgentUiSession, preview_id};
use crate::config::persist_tui_theme_selection;
use crate::interaction::{
    ApprovalPrompt, ModelReasoningEffortOutcome, PermissionProfile, PermissionRequestDecision,
    PermissionRequestPrompt, SessionPermissionMode, UserInputAnswer, UserInputPrompt,
    UserInputSubmission,
};
use crate::statusline::status_line_fields;
use crate::theme::{ThemeCatalog, active_theme_id, install_theme_catalog, set_active_theme};
use crate::ui::{
    HistoryRollbackRound, LiveTaskAttentionAction, LiveTaskAttentionOutcome, LiveTaskControlAction,
    LiveTaskControlOutcome, LiveTaskMessageAction, LiveTaskMessageOutcome, LiveTaskSpawnOutcome,
    LiveTaskSummary, LiveTaskWaitOutcome, LoadedAgentSession, LoadedMcpPrompt, LoadedMcpResource,
    LoadedSession, LoadedTask, McpPromptSummary, McpResourceSummary, McpServerSummary,
    PersistedAgentSessionSummary, PersistedSessionSearchMatch, PersistedSessionSummary,
    PersistedTaskSummary, SessionEvent, SessionExportArtifact, SessionOperation,
    SessionOperationAction, SessionOperationOutcome, SessionStartupSnapshot, SideQuestionOutcome,
    StartupDiagnosticsSnapshot, UIAsyncCommand, UIAsyncValue, UICommand, UIQuery, UIQueryValue,
    UIResultValue,
};
use approval::approval_decision_for_key;
use commands::{
    SlashCommand, SlashCommandEnterAction, command_palette_lines_for, cycle_slash_command,
    move_slash_command_selection, parse_slash_command, resolve_slash_enter_action,
};
use history::{
    format_agent_session_inspector, format_agent_session_summary_collection,
    format_live_task_control_outcome, format_live_task_message_outcome,
    format_live_task_spawn_outcome, format_live_task_summary_line, format_live_task_wait_outcome,
    format_mcp_prompt_summary_line, format_mcp_resource_summary_line,
    format_mcp_server_summary_line, format_session_export_result, format_session_inspector,
    format_session_operation_outcome, format_session_search_collection,
    format_session_summary_collection, format_session_transcript_lines, format_startup_diagnostics,
    format_task_inspector, format_task_summary_collection, format_visible_transcript_lines,
    format_visible_transcript_preview_lines,
};
use observer::SharedRenderObserver;
use operator_support::*;
use paste_burst::{CharDecision, FlushResult, PasteBurst};
use render::{main_pane_viewport_height, render};
pub use state::SharedUiState;
use state::{
    ComposerDraftAttachmentKind, ComposerDraftAttachmentState, ComposerDraftState,
    ComposerSubmission, InspectorEntry, ToastTone, TranscriptEntry, TranscriptShellDetail,
    TuiState,
};
use tool_state::restore_tool_panels;

use agent::RuntimeCommand;
use agent::tools::{
    ToolExecutionContext, load_tool_image, resolve_tool_path_against_workspace_root,
};
use agent::types::{
    AgentStatus, Message, MessagePart, SubmittedPromptSnapshot, message_operator_text,
};
use anyhow::{Context, Result, anyhow};
use base64::Engine;
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use std::collections::BTreeMap;
use std::env;
use std::fs as stdfs;
use std::io::{self, Stdout};
use std::path::Path;
use std::process::Command as ProcessCommand;
use std::time::Instant;
use tempfile::NamedTempFile;
use tokio::fs;
use tokio::task::{JoinHandle, spawn_local};
use tokio::time::{Duration, sleep};
use tracing::error;

pub struct CodeAgentTui {
    session: CodeAgentUiSession,
    initial_prompt: Option<String>,
    ui_state: SharedUiState,
    event_renderer: SharedRenderObserver,
    active_user_input: Option<ActiveUserInputState>,
    turn_task: Option<JoinHandle<Result<()>>>,
    operator_task: Option<JoinHandle<Result<OperatorTaskOutcome>>>,
    paste_burst: PasteBurst,
}

enum OperatorTaskOutcome {
    WaitLiveTask(LiveTaskWaitOutcome),
    SideQuestion(SideQuestionOutcome),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlainInputSubmitAction {
    StartPrompt,
    QueuePrompt,
    SteerActiveTurn,
}

const LARGE_PASTE_CHAR_THRESHOLD: usize = 1000;

#[derive(Clone, Debug, Default)]
struct ActiveUserInputState {
    prompt_id: String,
    current_question: usize,
    answers: BTreeMap<String, UserInputAnswer>,
    collecting_other_note: bool,
}

impl ActiveUserInputState {
    fn new(prompt_id: String) -> Self {
        Self {
            prompt_id,
            ..Self::default()
        }
    }
}

struct UserInputView<'a> {
    prompt: &'a UserInputPrompt,
    flow: Option<&'a ActiveUserInputState>,
    input: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LoadedComposerFile {
    requested_path: String,
    file_name: Option<String>,
    mime_type: Option<String>,
    data_base64: String,
}

fn summarize_nonfatal_error(operation: &'static str, error: &anyhow::Error) -> String {
    error!(operation, error = ?error, "UI operation failed");
    error.to_string()
}

impl CodeAgentTui {
    pub fn new(
        session: CodeAgentUiSession,
        initial_prompt: Option<String>,
        ui_state: SharedUiState,
        theme_catalog: ThemeCatalog,
    ) -> Self {
        install_theme_catalog(theme_catalog);
        Self {
            session,
            initial_prompt,
            event_renderer: SharedRenderObserver::new(ui_state.clone()),
            ui_state,
            active_user_input: None,
            turn_task: None,
            operator_task: None,
            paste_burst: PasteBurst::default(),
        }
    }
}

#[cfg(test)]
mod tests;
