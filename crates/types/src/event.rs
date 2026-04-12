use crate::{
    AgentId, AgentSessionId, CallId, ContextWindowUsage, EnvelopeId, EventId, HookEvent,
    HookResult, Message, MessageId, MessagePart, MonitorId, Reasoning, ResponseId, SessionId,
    TaskId, TokenLedgerSnapshot, TokenUsage, TokenUsagePhase, ToolCall, ToolCallId, ToolName,
    ToolSpec, TurnId, WorktreeId,
};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::fmt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum ProviderContinuation {
    OpenAiResponses { response_id: ResponseId },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Queued,
    Running,
    WaitingApproval,
    WaitingMessage,
    Completed,
    Failed,
    Cancelled,
}

impl AgentStatus {
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

impl fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::WaitingApproval => "waiting_approval",
            Self::WaitingMessage => "waiting_message",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        };
        f.write_str(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Open,
    Queued,
    Running,
    WaitingApproval,
    WaitingMessage,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Open => "open",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::WaitingApproval => "waiting_approval",
            Self::WaitingMessage => "waiting_message",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        };
        f.write_str(value)
    }
}

impl From<AgentStatus> for TaskStatus {
    fn from(value: AgentStatus) -> Self {
        match value {
            AgentStatus::Queued => Self::Queued,
            AgentStatus::Running => Self::Running,
            AgentStatus::WaitingApproval => Self::WaitingApproval,
            AgentStatus::WaitingMessage => Self::WaitingMessage,
            AgentStatus::Completed => Self::Completed,
            AgentStatus::Failed => Self::Failed,
            AgentStatus::Cancelled => Self::Cancelled,
        }
    }
}

impl From<&AgentStatus> for TaskStatus {
    fn from(value: &AgentStatus) -> Self {
        Self::from(value.clone())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MonitorStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl MonitorStatus {
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

impl fmt::Display for MonitorStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        };
        f.write_str(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MonitorStream {
    Stdout,
    Stderr,
}

impl fmt::Display for MonitorStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stdout => f.write_str("stdout"),
            Self::Stderr => f.write_str("stderr"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeScope {
    Session,
    ChildAgent,
}

impl fmt::Display for WorktreeScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Session => "session",
            Self::ChildAgent => "child_agent",
        };
        f.write_str(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeStatus {
    Active,
    Inactive,
    Removed,
}

impl WorktreeStatus {
    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active)
    }
}

impl fmt::Display for WorktreeStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
            Self::Removed => "removed",
        };
        f.write_str(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskOrigin {
    UserCreated,
    AgentCreated,
    ChildAgentBacked,
    AutomationBacked,
}

impl fmt::Display for TaskOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::UserCreated => "user_created",
            Self::AgentCreated => "agent_created",
            Self::ChildAgentBacked => "child_agent_backed",
            Self::AutomationBacked => "automation_backed",
        };
        f.write_str(value)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentInputDelivery {
    #[default]
    Queue,
    Interrupt,
}

impl fmt::Display for AgentInputDelivery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Queue => "queue",
            Self::Interrupt => "interrupt",
        };
        f.write_str(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentHandle {
    pub agent_id: AgentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<AgentId>,
    pub session_id: SessionId,
    pub agent_session_id: AgentSessionId,
    pub task_id: TaskId,
    pub role: String,
    pub status: AgentStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<WorktreeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_root: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentArtifact {
    pub kind: String,
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentTaskSpec {
    pub task_id: TaskId,
    pub role: String,
    pub prompt: String,
    pub origin: TaskOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steer: Option<String>,
    #[serde(default)]
    pub allowed_tools: Vec<ToolName>,
    #[serde(default)]
    pub requested_write_set: Vec<String>,
    #[serde(default)]
    pub dependency_ids: Vec<TaskId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentResultEnvelope {
    pub agent_id: AgentId,
    pub task_id: TaskId,
    pub status: AgentStatus,
    pub summary: String,
    pub text: String,
    #[serde(default)]
    pub artifacts: Vec<AgentArtifact>,
    #[serde(default)]
    pub claimed_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_payload: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TaskSummaryRecord {
    pub task_id: TaskId,
    pub session_id: SessionId,
    pub agent_session_id: AgentSessionId,
    pub role: String,
    pub origin: TaskOrigin,
    pub status: TaskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_agent_id: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<WorktreeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_root: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskRecord {
    pub summary: TaskSummaryRecord,
    pub spec: AgentTaskSpec,
    #[serde(default)]
    pub claimed_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<AgentResultEnvelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MonitorSummaryRecord {
    pub monitor_id: MonitorId,
    pub session_id: SessionId,
    pub agent_session_id: AgentSessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
    pub command: String,
    pub cwd: String,
    pub shell: String,
    pub login: bool,
    pub status: MonitorStatus,
    pub started_at_unix_s: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_unix_s: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorktreeSummaryRecord {
    pub worktree_id: WorktreeId,
    pub session_id: SessionId,
    pub agent_session_id: AgentSessionId,
    pub scope: WorktreeScope,
    pub status: WorktreeStatus,
    pub root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_agent_id: Option<AgentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub created_at_unix_s: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_unix_s: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MonitorEventKind {
    Line {
        stream: MonitorStream,
        text: String,
    },
    StateChanged {
        status: MonitorStatus,
    },
    Completed {
        exit_code: i32,
    },
    Failed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Cancelled {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MonitorEventRecord {
    pub monitor_id: MonitorId,
    pub timestamp_unix_s: u64,
    pub kind: MonitorEventKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentEnvelopeKind {
    SpawnRequested {
        task: AgentTaskSpec,
    },
    Started {
        task: AgentTaskSpec,
    },
    StatusChanged {
        status: AgentStatus,
    },
    Input {
        message: Message,
        #[serde(default)]
        delivery: AgentInputDelivery,
    },
    Artifact {
        artifact: AgentArtifact,
    },
    ClaimRequested {
        files: Vec<String>,
    },
    ClaimGranted {
        files: Vec<String>,
    },
    ClaimRejected {
        files: Vec<String>,
        owner: AgentId,
    },
    Result {
        result: AgentResultEnvelope,
    },
    Failed {
        error: String,
    },
    Cancelled {
        reason: Option<String>,
    },
    Heartbeat,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentEnvelope {
    pub envelope_id: EnvelopeId,
    pub agent_id: AgentId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<AgentId>,
    pub session_id: SessionId,
    pub agent_session_id: AgentSessionId,
    pub timestamp_ms: u64,
    pub kind: AgentEnvelopeKind,
}

impl AgentEnvelope {
    #[must_use]
    pub fn new(
        agent_id: AgentId,
        parent_agent_id: Option<AgentId>,
        session_id: SessionId,
        agent_session_id: AgentSessionId,
        kind: AgentEnvelopeKind,
    ) -> Self {
        Self {
            envelope_id: EnvelopeId::new(),
            agent_id,
            parent_agent_id,
            session_id,
            agent_session_id,
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |value| {
                    value
                        .as_millis()
                        .min(u128::from(u64::MAX))
                        .try_into()
                        .unwrap_or(u64::MAX)
                }),
            kind,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentWaitMode {
    Any,
    All,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentWaitRequest {
    pub agent_ids: Vec<AgentId>,
    #[serde(default = "default_agent_wait_mode")]
    pub mode: AgentWaitMode,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentWaitResponse {
    pub completed: Vec<AgentHandle>,
    pub pending: Vec<AgentHandle>,
    #[serde(default)]
    pub results: Vec<AgentResultEnvelope>,
}

fn default_agent_wait_mode() -> AgentWaitMode {
    AgentWaitMode::All
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    pub session_id: SessionId,
    pub agent_session_id: AgentSessionId,
    pub turn_id: TurnId,
    pub instructions: Vec<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub additional_context: Vec<String>,
    #[serde(default)]
    pub continuation: Option<ProviderContinuation>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubmittedPromptAttachmentKind {
    Paste {
        text: String,
    },
    LocalImage {
        requested_path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
    RemoteImage {
        requested_url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
    LocalFile {
        requested_path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
    RemoteFile {
        requested_url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
    EmbeddedImage {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
    EmbeddedFile {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uri: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubmittedPromptAttachment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(flatten)]
    pub kind: SubmittedPromptAttachmentKind,
}

impl SubmittedPromptAttachment {
    #[must_use]
    pub fn from_message_part(part: &MessagePart) -> Option<Self> {
        match part {
            MessagePart::Paste { label, text } => Some(Self {
                placeholder: Some(label.clone()),
                kind: SubmittedPromptAttachmentKind::Paste { text: text.clone() },
            }),
            MessagePart::Image { mime_type, .. } => Some(Self {
                placeholder: None,
                kind: SubmittedPromptAttachmentKind::EmbeddedImage {
                    mime_type: Some(mime_type.clone()),
                },
            }),
            MessagePart::ImageUrl { url, mime_type } => Some(Self {
                placeholder: None,
                kind: SubmittedPromptAttachmentKind::RemoteImage {
                    requested_url: url.clone(),
                    mime_type: mime_type.clone(),
                },
            }),
            MessagePart::File {
                file_name,
                mime_type,
                uri,
                ..
            } => Some(Self {
                placeholder: None,
                kind: match uri {
                    Some(uri) if uri.starts_with("http://") || uri.starts_with("https://") => {
                        SubmittedPromptAttachmentKind::RemoteFile {
                            requested_url: uri.clone(),
                            file_name: file_name.clone(),
                            mime_type: mime_type.clone(),
                        }
                    }
                    Some(uri) => SubmittedPromptAttachmentKind::LocalFile {
                        requested_path: uri.clone(),
                        file_name: file_name.clone(),
                        mime_type: mime_type.clone(),
                    },
                    None => SubmittedPromptAttachmentKind::EmbeddedFile {
                        file_name: file_name.clone(),
                        mime_type: mime_type.clone(),
                        uri: None,
                    },
                },
            }),
            _ => None,
        }
    }

    #[must_use]
    pub fn preview_text(&self) -> String {
        match &self.kind {
            SubmittedPromptAttachmentKind::Paste { .. } => self
                .placeholder
                .clone()
                .unwrap_or_else(|| "[Paste]".to_string()),
            SubmittedPromptAttachmentKind::LocalImage { requested_path, .. } => {
                format!("image {}", requested_path)
            }
            SubmittedPromptAttachmentKind::RemoteImage { requested_url, .. } => {
                format!("image {}", requested_url)
            }
            SubmittedPromptAttachmentKind::LocalFile { requested_path, .. } => {
                format!("file {}", requested_path)
            }
            SubmittedPromptAttachmentKind::RemoteFile { requested_url, .. } => {
                format!("file {}", requested_url)
            }
            SubmittedPromptAttachmentKind::EmbeddedImage { mime_type } => {
                format!("image {}", mime_type.as_deref().unwrap_or("embedded"))
            }
            SubmittedPromptAttachmentKind::EmbeddedFile {
                file_name,
                mime_type,
                uri,
            } => format!(
                "file {}",
                file_name
                    .as_deref()
                    .or(uri.as_deref())
                    .or(mime_type.as_deref())
                    .unwrap_or("embedded")
            ),
        }
    }

    #[must_use]
    pub fn search_strings(&self) -> Vec<String> {
        let mut values = vec![self.preview_text()];
        match &self.kind {
            SubmittedPromptAttachmentKind::Paste { text } => values.push(text.clone()),
            SubmittedPromptAttachmentKind::LocalImage { requested_path, .. }
            | SubmittedPromptAttachmentKind::LocalFile { requested_path, .. } => {
                values.push(requested_path.clone());
            }
            SubmittedPromptAttachmentKind::RemoteImage { requested_url, .. }
            | SubmittedPromptAttachmentKind::RemoteFile { requested_url, .. } => {
                values.push(requested_url.clone());
            }
            SubmittedPromptAttachmentKind::EmbeddedImage { .. } => {}
            SubmittedPromptAttachmentKind::EmbeddedFile {
                file_name,
                mime_type,
                uri,
            } => {
                if let Some(file_name) = file_name {
                    values.push(file_name.clone());
                }
                if let Some(mime_type) = mime_type {
                    values.push(mime_type.clone());
                }
                if let Some(uri) = uri {
                    values.push(uri.clone());
                }
            }
        }
        values
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SubmittedPromptSnapshot {
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<SubmittedPromptAttachment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
enum SubmittedPromptSnapshotWire {
    Legacy(String),
    Snapshot(SubmittedPromptSnapshotData),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
struct SubmittedPromptSnapshotData {
    #[serde(default)]
    text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    attachments: Vec<SubmittedPromptAttachment>,
}

impl From<SubmittedPromptSnapshotData> for SubmittedPromptSnapshot {
    fn from(value: SubmittedPromptSnapshotData) -> Self {
        Self {
            text: value.text,
            attachments: value.attachments,
        }
    }
}

impl<'de> Deserialize<'de> for SubmittedPromptSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match SubmittedPromptSnapshotWire::deserialize(deserializer)? {
            SubmittedPromptSnapshotWire::Legacy(text) => Ok(Self::from_text(text)),
            SubmittedPromptSnapshotWire::Snapshot(snapshot) => Ok(snapshot.into()),
        }
    }
}

impl SubmittedPromptSnapshot {
    #[must_use]
    pub fn from_text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            attachments: Vec::new(),
        }
    }

    #[must_use]
    pub fn from_message(message: &Message) -> Self {
        Self {
            text: message.text_content(),
            attachments: message
                .parts
                .iter()
                .filter_map(SubmittedPromptAttachment::from_message_part)
                .collect(),
        }
    }

    #[must_use]
    pub fn preview_text(&self) -> String {
        let trimmed = self.text.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
        self.attachments
            .iter()
            .map(SubmittedPromptAttachment::preview_text)
            .collect::<Vec<_>>()
            .join(" · ")
    }

    #[must_use]
    pub fn search_strings(&self) -> Vec<String> {
        let mut values = Vec::new();
        if !self.text.trim().is_empty() {
            values.push(self.text.clone());
        }
        values.extend(
            self.attachments
                .iter()
                .flat_map(SubmittedPromptAttachment::search_strings),
        );
        values
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionSummaryTokenUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<ContextWindowUsage>,
    #[serde(default)]
    pub cumulative_usage: TokenUsage,
}

impl SessionSummaryTokenUsage {
    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.context_window.is_none() && self.cumulative_usage.is_zero()
    }
}

impl From<TokenLedgerSnapshot> for SessionSummaryTokenUsage {
    fn from(value: TokenLedgerSnapshot) -> Self {
        Self {
            context_window: value.context_window,
            cumulative_usage: value.cumulative_usage,
        }
    }
}

impl From<&TokenLedgerSnapshot> for SessionSummaryTokenUsage {
    fn from(value: &TokenLedgerSnapshot) -> Self {
        Self {
            context_window: value.context_window,
            cumulative_usage: value.cumulative_usage,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelEvent {
    TextDelta {
        delta: String,
    },
    ToolCallRequested {
        call: ToolCall,
    },
    ResponseComplete {
        stop_reason: Option<String>,
        #[serde(default)]
        message_id: Option<MessageId>,
        #[serde(default)]
        continuation: Option<ProviderContinuation>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<TokenUsage>,
        reasoning: Vec<Reasoning>,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum ToolLifecycleEventKind {
    Started {
        call: ToolCall,
    },
    Completed {
        call: ToolCall,
        output: crate::ToolResult,
    },
    Failed {
        call: ToolCall,
        error: String,
    },
    Cancelled {
        call: ToolCall,
        reason: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolLifecycleEventEnvelope {
    pub id: EventId,
    pub session_id: SessionId,
    pub agent_session_id: AgentSessionId,
    pub turn_id: Option<TurnId>,
    pub tool_call_id: ToolCallId,
    pub call_id: CallId,
    pub tool_name: ToolName,
    pub timestamp_ms: u128,
    pub event: ToolLifecycleEventKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEventKind {
    SessionStart {
        reason: Option<String>,
    },
    InstructionsLoaded {
        count: usize,
    },
    SteerApplied {
        message: String,
        reason: Option<String>,
    },
    UserPromptSubmit {
        prompt: SubmittedPromptSnapshot,
    },
    ModelRequestStarted {
        request: ModelRequest,
    },
    CompactionCompleted {
        reason: String,
        source_message_count: usize,
        retained_message_count: usize,
        summary_chars: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary_message_id: Option<MessageId>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        retained_tail_message_ids: Vec<MessageId>,
    },
    ModelResponseCompleted {
        assistant_text: String,
        tool_calls: Vec<ToolCall>,
        #[serde(default)]
        continuation: Option<ProviderContinuation>,
    },
    TokenUsageUpdated {
        phase: TokenUsagePhase,
        ledger: TokenLedgerSnapshot,
    },
    HookInvoked {
        hook_name: String,
        event: HookEvent,
    },
    HookCompleted {
        hook_name: String,
        event: HookEvent,
        output: HookResult,
    },
    TranscriptMessage {
        message: Message,
    },
    TranscriptMessagePatched {
        message_id: MessageId,
        message: Message,
    },
    TranscriptMessageRemoved {
        message_id: MessageId,
    },
    ToolApprovalRequested {
        call: ToolCall,
        reasons: Vec<String>,
    },
    ToolApprovalResolved {
        call: ToolCall,
        approved: bool,
        reason: Option<String>,
    },
    ToolCallStarted {
        call: ToolCall,
    },
    ToolCallCompleted {
        call: ToolCall,
        output: crate::ToolResult,
    },
    ToolCallFailed {
        call: ToolCall,
        error: String,
    },
    Notification {
        source: String,
        message: String,
    },
    MonitorStarted {
        summary: MonitorSummaryRecord,
    },
    WorktreeEntered {
        summary: WorktreeSummaryRecord,
    },
    WorktreeUpdated {
        summary: WorktreeSummaryRecord,
    },
    MonitorEvent {
        event: MonitorEventRecord,
    },
    MonitorUpdated {
        summary: MonitorSummaryRecord,
    },
    TaskCreated {
        task: AgentTaskSpec,
        parent_agent_id: Option<AgentId>,
        status: TaskStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    TaskUpdated {
        task_id: TaskId,
        status: TaskStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    TaskCompleted {
        task_id: TaskId,
        agent_id: AgentId,
        status: TaskStatus,
    },
    SubagentStart {
        handle: AgentHandle,
        task: AgentTaskSpec,
    },
    AgentEnvelope {
        envelope: AgentEnvelope,
    },
    SubagentStop {
        handle: AgentHandle,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<AgentResultEnvelope>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Stop {
        reason: Option<String>,
    },
    StopFailure {
        reason: Option<String>,
    },
    SessionEnd {
        reason: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionEventEnvelope {
    pub id: EventId,
    pub session_id: SessionId,
    pub agent_session_id: AgentSessionId,
    pub turn_id: Option<TurnId>,
    pub tool_call_id: Option<ToolCallId>,
    pub timestamp_ms: u128,
    pub event: SessionEventKind,
}

impl SessionEventEnvelope {
    #[must_use]
    pub fn new(
        session_id: SessionId,
        agent_session_id: AgentSessionId,
        turn_id: Option<TurnId>,
        tool_call_id: Option<ToolCallId>,
        event: SessionEventKind,
    ) -> Self {
        Self {
            id: EventId::new(),
            session_id,
            agent_session_id,
            turn_id,
            tool_call_id,
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |value| value.as_millis()),
            event,
        }
    }

    #[must_use]
    pub fn tool_lifecycle_event(&self) -> Option<ToolLifecycleEventEnvelope> {
        let lifecycle = match &self.event {
            SessionEventKind::ToolCallStarted { call } => {
                ToolLifecycleEventKind::Started { call: call.clone() }
            }
            SessionEventKind::ToolCallCompleted { call, output } => {
                ToolLifecycleEventKind::Completed {
                    call: call.clone(),
                    output: output.clone(),
                }
            }
            SessionEventKind::ToolCallFailed { call, error } => ToolLifecycleEventKind::Failed {
                call: call.clone(),
                error: error.clone(),
            },
            _ => return None,
        };

        let (tool_call_id, call_id, tool_name) = match &lifecycle {
            ToolLifecycleEventKind::Started { call }
            | ToolLifecycleEventKind::Completed { call, .. }
            | ToolLifecycleEventKind::Failed { call, .. }
            | ToolLifecycleEventKind::Cancelled { call, .. } => (
                self.tool_call_id.clone().unwrap_or_else(|| call.id.clone()),
                call.call_id.clone(),
                call.tool_name.clone(),
            ),
        };

        Some(ToolLifecycleEventEnvelope {
            id: self.id.clone(),
            session_id: self.session_id.clone(),
            agent_session_id: self.agent_session_id.clone(),
            turn_id: self.turn_id.clone(),
            tool_call_id,
            call_id,
            tool_name,
            timestamp_ms: self.timestamp_ms,
            event: lifecycle,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AgentEnvelope, AgentEnvelopeKind, AgentResultEnvelope, AgentStatus, AgentTaskSpec,
        SessionEventEnvelope, SessionEventKind, SubmittedPromptAttachment,
        SubmittedPromptAttachmentKind, SubmittedPromptSnapshot, TaskOrigin, TaskStatus,
        ToolLifecycleEventKind,
    };
    use crate::{AgentId, ToolCall, ToolCallId, ToolName, ToolOrigin, ToolResult};

    #[test]
    fn run_event_maps_tool_completion_to_lifecycle_envelope() {
        let call = ToolCall {
            id: ToolCallId::new(),
            call_id: "call-read-1".into(),
            tool_name: "read".into(),
            arguments: serde_json::json!({"path":"sample.txt"}),
            origin: ToolOrigin::Local,
        };
        let output =
            ToolResult::text(call.id.clone(), "read", "ok").with_call_id(call.call_id.clone());
        let envelope = SessionEventEnvelope::new(
            "run_1".into(),
            "session_1".into(),
            Some("turn_1".into()),
            Some(call.id.clone()),
            SessionEventKind::ToolCallCompleted {
                call: call.clone(),
                output: output.clone(),
            },
        );

        let lifecycle = envelope
            .tool_lifecycle_event()
            .expect("tool lifecycle event");
        assert_eq!(lifecycle.id, envelope.id);
        assert_eq!(lifecycle.tool_call_id, call.id);
        assert_eq!(lifecycle.call_id, call.call_id);
        assert_eq!(lifecycle.tool_name, ToolName::from("read"));
        assert!(matches!(
            lifecycle.event,
            ToolLifecycleEventKind::Completed { call: mapped_call, output: mapped_output }
                if mapped_call.tool_name == ToolName::from("read") && mapped_output.text_content() == "ok"
        ));
    }

    #[test]
    fn agent_envelope_retains_parent_relation_and_result_payload() {
        let task = AgentTaskSpec {
            task_id: "task_1".into(),
            role: "reviewer".to_string(),
            prompt: "inspect".to_string(),
            origin: TaskOrigin::ChildAgentBacked,
            steer: None,
            allowed_tools: vec!["read".into()],
            requested_write_set: vec!["src/lib.rs".to_string()],
            dependency_ids: Vec::new(),
            timeout_seconds: Some(30),
        };
        let envelope = AgentEnvelope::new(
            AgentId::from("agent_child"),
            Some(AgentId::from("agent_parent")),
            "run_child".into(),
            "session_child".into(),
            AgentEnvelopeKind::Result {
                result: AgentResultEnvelope {
                    agent_id: AgentId::from("agent_child"),
                    task_id: "task_1".into(),
                    status: AgentStatus::Completed,
                    summary: "done".to_string(),
                    text: "ok".to_string(),
                    artifacts: Vec::new(),
                    claimed_files: vec!["src/lib.rs".to_string()],
                    structured_payload: Some(serde_json::json!({"task_id": task.task_id})),
                },
            },
        );

        assert_eq!(
            envelope.parent_agent_id,
            Some(AgentId::from("agent_parent"))
        );
        match envelope.kind {
            AgentEnvelopeKind::Result { result } => {
                assert_eq!(result.status, AgentStatus::Completed);
                assert_eq!(result.structured_payload.unwrap()["task_id"], "task_1");
            }
            other => panic!("unexpected envelope kind: {other:?}"),
        }
    }

    #[test]
    fn task_completed_event_keeps_agent_status() {
        let event = SessionEventEnvelope::new(
            "run_1".into(),
            "session_1".into(),
            None,
            None,
            SessionEventKind::TaskCompleted {
                task_id: "task_1".into(),
                agent_id: "agent_1".into(),
                status: TaskStatus::Cancelled,
            },
        );

        assert!(matches!(
            event.event,
            SessionEventKind::TaskCompleted {
                status: TaskStatus::Cancelled,
                ..
            }
        ));
    }

    #[test]
    fn user_prompt_submit_deserializes_legacy_string_payloads() {
        let event: SessionEventKind = serde_json::from_value(serde_json::json!({
            "kind": "user_prompt_submit",
            "prompt": "inspect the failure"
        }))
        .unwrap();

        assert_eq!(
            event,
            SessionEventKind::UserPromptSubmit {
                prompt: SubmittedPromptSnapshot::from_text("inspect the failure")
            }
        );
    }

    #[test]
    fn user_prompt_submit_serializes_rich_prompt_snapshots_as_objects() {
        let event = SessionEventKind::UserPromptSubmit {
            prompt: SubmittedPromptSnapshot {
                text: "[File #1]\nsummarize the report".to_string(),
                attachments: vec![SubmittedPromptAttachment {
                    placeholder: Some("[File #1]".to_string()),
                    kind: SubmittedPromptAttachmentKind::LocalFile {
                        requested_path: "reports/run.pdf".to_string(),
                        file_name: Some("run.pdf".to_string()),
                        mime_type: Some("application/pdf".to_string()),
                    },
                }],
            },
        };

        let encoded = serde_json::to_value(&event).unwrap();
        assert_eq!(encoded["kind"], "user_prompt_submit");
        assert_eq!(encoded["prompt"]["text"], "[File #1]\nsummarize the report");
        assert_eq!(encoded["prompt"]["attachments"][0]["kind"], "local_file");
        assert_eq!(
            encoded["prompt"]["attachments"][0]["requested_path"],
            "reports/run.pdf"
        );
    }
}
