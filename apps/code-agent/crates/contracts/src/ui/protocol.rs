use super::{
    events::SessionEvent,
    mcp::{
        LoadedMcpPrompt, LoadedMcpResource, McpPromptSummary, McpResourceSummary, McpServerSummary,
        StartupDiagnosticsSnapshot,
    },
    session::{
        HistoryRollbackOutcome, HistoryRollbackRound, LoadedAgentSession, LoadedSession,
        PersistedAgentSessionSummary, PersistedSessionSearchMatch, PersistedSessionSummary,
        SessionExportArtifact, SessionOperation, SessionOperationOutcome, SessionStartupSnapshot,
        SideQuestionOutcome,
    },
    tasks::{
        LiveTaskAttentionOutcome, LiveTaskControlOutcome, LiveTaskMessageOutcome,
        LiveTaskSpawnOutcome, LiveTaskSummary, LiveTaskWaitOutcome, LoadedTask,
        PersistedTaskSummary,
    },
};
use crate::interaction::{
    ApprovalDecision, ApprovalPrompt, ModelReasoningEffortOutcome, PendingControlSummary,
    PermissionProfile, PermissionRequestDecision, PermissionRequestPrompt, SessionPermissionMode,
    SessionPermissionModeOutcome, SkillSummary, UserInputPrompt, UserInputSubmission,
};
use agent::RuntimeCommand;
use agent::types::{Message, SubmittedPromptSnapshot};
use std::path::PathBuf;

pub type UIEvent = SessionEvent;

#[derive(Clone, Debug)]
pub enum UIQuery {
    WorkspaceRoot,
    StartupSnapshot,
    HostProcessSurfacesAllowed,
    ApprovalPrompt,
    PermissionRequestPrompt,
    UserInputPrompt,
    PendingControls,
    QueuedCommandCount,
    StartupDiagnostics,
    PermissionGrantProfiles,
    Skills,
}

#[derive(Debug)]
pub enum UIQueryResult {
    PathBuf(PathBuf),
    StartupSnapshot(SessionStartupSnapshot),
    Bool(bool),
    ApprovalPrompt(Option<ApprovalPrompt>),
    PermissionRequestPrompt(Option<PermissionRequestPrompt>),
    UserInputPrompt(Option<UserInputPrompt>),
    PendingControls(Vec<PendingControlSummary>),
    Usize(usize),
    StartupDiagnostics(StartupDiagnosticsSnapshot),
    PermissionGrantProfiles((PermissionProfile, PermissionProfile)),
    Skills(Vec<SkillSummary>),
}

pub trait UIQueryValue: Sized {
    fn from_query_result(result: UIQueryResult) -> Self;
}

#[derive(Clone, Debug)]
pub enum UICommand {
    ResolveApproval(ApprovalDecision),
    ResolvePermissionRequest(PermissionRequestDecision),
    ResolveUserInput(UserInputSubmission),
    CancelUserInput {
        reason: String,
    },
    RemovePendingControl {
        control_ref: String,
    },
    UpdatePendingControl {
        control_ref: String,
        content: String,
    },
    ScheduleRuntimeSteer {
        message: String,
        reason: Option<String>,
    },
    TakePendingSteers,
    CycleModelReasoningEffort,
    SetModelReasoningEffort {
        effort: String,
    },
    DrainEvents,
    ScheduleLiveTaskAttention {
        outcome: LiveTaskWaitOutcome,
        turn_running: bool,
    },
}

#[derive(Debug)]
pub enum UIResult {
    Bool(bool),
    PendingControl(PendingControlSummary),
    PendingControls(Vec<PendingControlSummary>),
    String(String),
    ModelReasoningEffortOutcome(ModelReasoningEffortOutcome),
    UIEvents(Vec<UIEvent>),
    LiveTaskAttentionOutcome(LiveTaskAttentionOutcome),
}

pub trait UIResultValue: Sized {
    fn from_ui_result(result: UIResult) -> Self;
}

#[derive(Clone, Debug)]
pub enum UIAsyncCommand {
    EndSession {
        reason: Option<String>,
    },
    ApplyControl {
        command: RuntimeCommand,
    },
    QueuePromptCommand {
        message: Message,
        submitted_prompt: Option<SubmittedPromptSnapshot>,
    },
    ClearQueuedCommands,
    DrainQueuedControls,
    RollbackVisibleHistoryToMessage {
        message_id: String,
    },
    HistoryRollbackRounds,
    CompactNow {
        notes: Option<String>,
    },
    ApplySessionOperation {
        operation: SessionOperation,
    },
    SetPermissionMode {
        mode: SessionPermissionMode,
    },
    RefreshStoredSessionCount,
    ListSessions,
    SearchSessions {
        query: String,
    },
    ListAgentSessions {
        session_ref: Option<String>,
    },
    ListTasks {
        session_ref: Option<String>,
    },
    ListLiveTasks,
    SpawnLiveTask {
        role: String,
        prompt: String,
    },
    SendLiveTask {
        task_or_agent_ref: String,
        message: String,
    },
    WaitLiveTask {
        task_or_agent_ref: String,
    },
    CancelLiveTask {
        task_or_agent_ref: String,
        reason: Option<String>,
    },
    LoadSession {
        session_ref: String,
    },
    LoadAgentSession {
        agent_session_ref: String,
    },
    LoadTask {
        task_ref: String,
    },
    ExportSession {
        session_ref: String,
        path: String,
    },
    ExportSessionTranscript {
        session_ref: String,
        path: String,
    },
    AnswerSideQuestion {
        question: String,
    },
    ListMcpServers,
    ListMcpPrompts,
    ListMcpResources,
    LoadMcpPrompt {
        server_name: String,
        prompt_name: String,
    },
    LoadMcpResource {
        server_name: String,
        uri: String,
    },
}

#[derive(Debug)]
pub enum UIAsyncResult {
    Unit(()),
    Bool(bool),
    String(String),
    Usize(usize),
    HistoryRollbackOutcome(HistoryRollbackOutcome),
    HistoryRollbackRounds(Vec<HistoryRollbackRound>),
    SessionOperationOutcome(SessionOperationOutcome),
    SessionPermissionModeOutcome(SessionPermissionModeOutcome),
    PersistedSessions(Vec<PersistedSessionSummary>),
    PersistedSessionSearchMatches(Vec<PersistedSessionSearchMatch>),
    PersistedAgentSessions(Vec<PersistedAgentSessionSummary>),
    PersistedTasks(Vec<PersistedTaskSummary>),
    LiveTasks(Vec<LiveTaskSummary>),
    LiveTaskSpawnOutcome(LiveTaskSpawnOutcome),
    LiveTaskMessageOutcome(LiveTaskMessageOutcome),
    LiveTaskWaitOutcome(LiveTaskWaitOutcome),
    LiveTaskControlOutcome(LiveTaskControlOutcome),
    LoadedSession(LoadedSession),
    LoadedAgentSession(LoadedAgentSession),
    LoadedTask(LoadedTask),
    SessionExportArtifact(SessionExportArtifact),
    SideQuestionOutcome(SideQuestionOutcome),
    McpServerSummaries(Vec<McpServerSummary>),
    McpPromptSummaries(Vec<McpPromptSummary>),
    McpResourceSummaries(Vec<McpResourceSummary>),
    LoadedMcpPrompt(LoadedMcpPrompt),
    LoadedMcpResource(LoadedMcpResource),
}

pub trait UIAsyncValue: Sized {
    fn from_ui_async_result(result: UIAsyncResult) -> Self;
}

macro_rules! impl_ui_query_value {
    ($ty:ty, $variant:ident) => {
        impl UIQueryValue for $ty {
            fn from_query_result(result: UIQueryResult) -> Self {
                match result {
                    UIQueryResult::$variant(value) => value,
                    other => panic!(
                        "ui query result mismatch: expected {}, got {:?}",
                        stringify!($variant),
                        other
                    ),
                }
            }
        }
    };
}

macro_rules! impl_ui_result_value {
    ($ty:ty, $variant:ident) => {
        impl UIResultValue for $ty {
            fn from_ui_result(result: UIResult) -> Self {
                match result {
                    UIResult::$variant(value) => value,
                    other => panic!(
                        "ui result mismatch: expected {}, got {:?}",
                        stringify!($variant),
                        other
                    ),
                }
            }
        }
    };
}

macro_rules! impl_ui_async_value {
    ($ty:ty, $variant:ident) => {
        impl UIAsyncValue for $ty {
            fn from_ui_async_result(result: UIAsyncResult) -> Self {
                match result {
                    UIAsyncResult::$variant(value) => value,
                    other => panic!(
                        "ui async result mismatch: expected {}, got {:?}",
                        stringify!($variant),
                        other
                    ),
                }
            }
        }
    };
}

impl_ui_query_value!(PathBuf, PathBuf);
impl_ui_query_value!(SessionStartupSnapshot, StartupSnapshot);
impl_ui_query_value!(bool, Bool);
impl_ui_query_value!(Option<ApprovalPrompt>, ApprovalPrompt);
impl_ui_query_value!(Option<PermissionRequestPrompt>, PermissionRequestPrompt);
impl_ui_query_value!(Option<UserInputPrompt>, UserInputPrompt);
impl_ui_query_value!(Vec<PendingControlSummary>, PendingControls);
impl_ui_query_value!(usize, Usize);
impl_ui_query_value!(StartupDiagnosticsSnapshot, StartupDiagnostics);
impl_ui_query_value!(
    (PermissionProfile, PermissionProfile),
    PermissionGrantProfiles
);
impl_ui_query_value!(Vec<SkillSummary>, Skills);

impl_ui_result_value!(bool, Bool);
impl_ui_result_value!(PendingControlSummary, PendingControl);
impl_ui_result_value!(Vec<PendingControlSummary>, PendingControls);
impl_ui_result_value!(String, String);
impl_ui_result_value!(ModelReasoningEffortOutcome, ModelReasoningEffortOutcome);
impl_ui_result_value!(Vec<UIEvent>, UIEvents);
impl_ui_result_value!(LiveTaskAttentionOutcome, LiveTaskAttentionOutcome);

impl_ui_async_value!((), Unit);
impl_ui_async_value!(bool, Bool);
impl_ui_async_value!(String, String);
impl_ui_async_value!(usize, Usize);
impl_ui_async_value!(HistoryRollbackOutcome, HistoryRollbackOutcome);
impl_ui_async_value!(Vec<HistoryRollbackRound>, HistoryRollbackRounds);
impl_ui_async_value!(SessionOperationOutcome, SessionOperationOutcome);
impl_ui_async_value!(SessionPermissionModeOutcome, SessionPermissionModeOutcome);
impl_ui_async_value!(Vec<PersistedSessionSummary>, PersistedSessions);
impl_ui_async_value!(
    Vec<PersistedSessionSearchMatch>,
    PersistedSessionSearchMatches
);
impl_ui_async_value!(Vec<PersistedAgentSessionSummary>, PersistedAgentSessions);
impl_ui_async_value!(Vec<PersistedTaskSummary>, PersistedTasks);
impl_ui_async_value!(Vec<LiveTaskSummary>, LiveTasks);
impl_ui_async_value!(LiveTaskSpawnOutcome, LiveTaskSpawnOutcome);
impl_ui_async_value!(LiveTaskMessageOutcome, LiveTaskMessageOutcome);
impl_ui_async_value!(LiveTaskWaitOutcome, LiveTaskWaitOutcome);
impl_ui_async_value!(LiveTaskControlOutcome, LiveTaskControlOutcome);
impl_ui_async_value!(LoadedSession, LoadedSession);
impl_ui_async_value!(LoadedAgentSession, LoadedAgentSession);
impl_ui_async_value!(LoadedTask, LoadedTask);
impl_ui_async_value!(SessionExportArtifact, SessionExportArtifact);
impl_ui_async_value!(SideQuestionOutcome, SideQuestionOutcome);
impl_ui_async_value!(Vec<McpServerSummary>, McpServerSummaries);
impl_ui_async_value!(Vec<McpPromptSummary>, McpPromptSummaries);
impl_ui_async_value!(Vec<McpResourceSummary>, McpResourceSummaries);
impl_ui_async_value!(LoadedMcpPrompt, LoadedMcpPrompt);
impl_ui_async_value!(LoadedMcpResource, LoadedMcpResource);
