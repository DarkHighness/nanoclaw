use crate::agent_mailbox::{AgentControlMessage, AgentMailboxReceiver, agent_mailbox_channel};
use crate::{
    AgentRuntime, AgentRuntimeBuilder, AgentSessionManager, CompactionConfig,
    ConversationCompactor, HookRunner, LoopDetectionConfig, ModelBackend, Result, RuntimeError,
    RuntimeSession, ToolApprovalHandler, ToolApprovalPolicy, WriteLeaseManager,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use skills::SkillCatalog;
use std::path::PathBuf;
use std::sync::Arc;
use store::RunStore;
use tools::{
    SubagentExecutor, SubagentParentContext, ToolError, ToolExecutionContext, ToolRegistry,
    resolve_tool_path_against_workspace_root,
};
use types::{
    AgentArtifact, AgentEnvelope, AgentEnvelopeKind, AgentHandle, AgentId, AgentResultEnvelope,
    AgentStatus, AgentTaskSpec, AgentWaitRequest, AgentWaitResponse, HookRegistration,
    RunEventEnvelope, RunEventKind, RunId, SessionId, ToolName,
};

const DEFAULT_EXCLUDED_CHILD_TOOLS: &[&str] = &[
    "task",
    "task_batch",
    "agent_spawn",
    "agent_send",
    "agent_wait",
    "agent_list",
    "agent_cancel",
    "todo_read",
    "todo_write",
];

#[derive(Clone)]
pub struct RuntimeSubagentExecutor {
    backend: Arc<dyn ModelBackend>,
    hook_runner: Arc<HookRunner>,
    store: Arc<dyn RunStore>,
    tool_registry: ToolRegistry,
    tool_context: ToolExecutionContext,
    tool_approval_handler: Arc<dyn ToolApprovalHandler>,
    tool_approval_policy: Arc<dyn ToolApprovalPolicy>,
    conversation_compactor: Arc<dyn ConversationCompactor>,
    compaction_config: CompactionConfig,
    loop_detection_config: LoopDetectionConfig,
    instructions: Vec<String>,
    hooks: Vec<HookRegistration>,
    skill_catalog: SkillCatalog,
    session_manager: AgentSessionManager,
    write_lease_manager: Arc<WriteLeaseManager>,
}

impl RuntimeSubagentExecutor {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        backend: Arc<dyn ModelBackend>,
        hook_runner: Arc<HookRunner>,
        store: Arc<dyn RunStore>,
        tool_registry: ToolRegistry,
        tool_context: ToolExecutionContext,
        tool_approval_handler: Arc<dyn ToolApprovalHandler>,
        tool_approval_policy: Arc<dyn ToolApprovalPolicy>,
        conversation_compactor: Arc<dyn ConversationCompactor>,
        compaction_config: CompactionConfig,
        loop_detection_config: LoopDetectionConfig,
        instructions: Vec<String>,
        hooks: Vec<HookRegistration>,
        skill_catalog: SkillCatalog,
    ) -> Self {
        Self {
            backend,
            hook_runner,
            store,
            tool_registry,
            tool_context,
            tool_approval_handler,
            tool_approval_policy,
            conversation_compactor,
            compaction_config,
            loop_detection_config,
            instructions,
            hooks,
            skill_catalog,
            session_manager: AgentSessionManager::new(),
            write_lease_manager: Arc::new(WriteLeaseManager::new()),
        }
    }

    fn resolve_child_tools(&self, requested: &[ToolName]) -> Result<(ToolRegistry, Vec<ToolName>)> {
        let allowed_names = if requested.is_empty() {
            self.tool_registry
                .names()
                .into_iter()
                .filter(|name| !DEFAULT_EXCLUDED_CHILD_TOOLS.contains(&name.as_str()))
                .collect::<Vec<_>>()
        } else {
            requested.to_vec()
        };
        let filtered = self.tool_registry.filtered_by_names(&allowed_names);
        let resolved_names = filtered.names();
        if !requested.is_empty() && resolved_names.is_empty() {
            return Err(RuntimeError::invalid_state(
                "agent_spawn: no allowed tools matched the parent registry",
            ));
        }
        Ok((filtered, resolved_names))
    }

    async fn append_parent_event(
        &self,
        parent: &SubagentParentContext,
        event: RunEventKind,
    ) -> Result<()> {
        let Some(run_id) = parent.run_id.clone() else {
            return Ok(());
        };
        let Some(session_id) = parent.session_id.clone() else {
            return Ok(());
        };
        self.store
            .append(RunEventEnvelope::new(
                run_id,
                session_id,
                parent.turn_id.clone(),
                None,
                event,
            ))
            .await
            .map_err(RuntimeError::from)
    }

    async fn append_agent_envelope(
        &self,
        parent: &SubagentParentContext,
        handle: &AgentHandle,
        kind: AgentEnvelopeKind,
    ) -> Result<()> {
        self.append_parent_event(
            parent,
            RunEventKind::AgentEnvelope {
                envelope: AgentEnvelope::new(
                    handle.agent_id.clone(),
                    handle.parent_agent_id.clone(),
                    handle.run_id.clone(),
                    handle.session_id.clone(),
                    kind,
                ),
            },
        )
        .await
    }

    fn resolve_write_set(&self, files: &[String]) -> std::result::Result<Vec<PathBuf>, ToolError> {
        files
            .iter()
            .map(|file| {
                resolve_tool_path_against_workspace_root(
                    file,
                    self.tool_context.effective_root(),
                    self.tool_context.container_workdir.as_deref(),
                )
            })
            .collect()
    }

    fn ensure_parent_can_access(
        &self,
        parent: &SubagentParentContext,
        handle: &AgentHandle,
    ) -> std::result::Result<(), ToolError> {
        if handle.parent_agent_id == parent.parent_agent_id {
            return Ok(());
        }
        let owner = handle
            .parent_agent_id
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "<root>".to_string());
        let caller = parent
            .parent_agent_id
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "<root>".to_string());
        Err(ToolError::invalid_state(format!(
            "parent agent {caller} cannot access child agent {} owned by {owner}",
            handle.agent_id
        )))
    }
}

#[async_trait]
impl SubagentExecutor for RuntimeSubagentExecutor {
    async fn spawn(
        &self,
        parent: SubagentParentContext,
        tasks: Vec<AgentTaskSpec>,
    ) -> std::result::Result<Vec<AgentHandle>, ToolError> {
        let mut handles = Vec::new();
        for task in tasks {
            let (tool_registry, resolved_tools) = self
                .resolve_child_tools(&task.allowed_tools)
                .map_err(|error| ToolError::invalid_state(error.to_string()))?;
            let task = AgentTaskSpec {
                allowed_tools: resolved_tools,
                ..task
            };

            let handle = AgentHandle {
                agent_id: AgentId::new(),
                parent_agent_id: parent.parent_agent_id.clone(),
                run_id: RunId::new(),
                session_id: SessionId::new(),
                task_id: task.task_id.clone(),
                role: task.role.clone(),
                status: AgentStatus::Queued,
            };

            self.append_parent_event(
                &parent,
                RunEventKind::TaskCreated {
                    task: task.clone(),
                    parent_agent_id: parent.parent_agent_id.clone(),
                },
            )
            .await
            .map_err(|error| ToolError::invalid_state(error.to_string()))?;
            self.append_agent_envelope(
                &parent,
                &handle,
                AgentEnvelopeKind::SpawnRequested { task: task.clone() },
            )
            .await
            .map_err(|error| ToolError::invalid_state(error.to_string()))?;

            let requested_files = self
                .resolve_write_set(&task.requested_write_set)
                .map_err(|error| ToolError::invalid_state(error.to_string()))?;
            if !task.requested_write_set.is_empty() {
                self.append_agent_envelope(
                    &parent,
                    &handle,
                    AgentEnvelopeKind::ClaimRequested {
                        files: task.requested_write_set.clone(),
                    },
                )
                .await
                .map_err(|error| ToolError::invalid_state(error.to_string()))?;
                match self
                    .write_lease_manager
                    .claim(&handle.agent_id, &requested_files)
                {
                    Ok(()) => {
                        self.append_agent_envelope(
                            &parent,
                            &handle,
                            AgentEnvelopeKind::ClaimGranted {
                                files: task.requested_write_set.clone(),
                            },
                        )
                        .await
                        .map_err(|error| ToolError::invalid_state(error.to_string()))?;
                    }
                    Err(conflict) => {
                        self.append_agent_envelope(
                            &parent,
                            &handle,
                            AgentEnvelopeKind::ClaimRejected {
                                files: task.requested_write_set.clone(),
                                owner: conflict.owner.clone(),
                            },
                        )
                        .await
                        .map_err(|error| ToolError::invalid_state(error.to_string()))?;
                        return Err(ToolError::invalid_state(format!(
                            "write lease conflict for {} owned by {}",
                            conflict.requested, conflict.owner
                        )));
                    }
                }
            }

            let runtime =
                AgentRuntimeBuilder::new(self.backend.clone(), self.store.clone())
                    .hook_runner(self.hook_runner.clone())
                    .tool_registry(tool_registry)
                    .tool_context(self.tool_context.clone().with_agent_scope(
                        handle.agent_id.clone(),
                        self.write_lease_manager.clone(),
                    ))
                    .tool_approval_handler(self.tool_approval_handler.clone())
                    .tool_approval_policy(self.tool_approval_policy.clone())
                    .conversation_compactor(self.conversation_compactor.clone())
                    .compaction_config(self.compaction_config.clone())
                    .loop_detection_config(self.loop_detection_config.clone())
                    .instructions(self.instructions.clone())
                    .hooks(self.hooks.clone())
                    .skill_catalog(self.skill_catalog.clone())
                    .session(RuntimeSession::new(
                        handle.run_id.clone(),
                        handle.session_id.clone(),
                    ))
                    .build();

            let (mailbox, mailbox_rx) = agent_mailbox_channel();
            self.session_manager
                .insert(handle.clone(), task.clone(), mailbox);

            let worker = ChildAgentWorker {
                parent: parent.clone(),
                store: self.store.clone(),
                session_manager: self.session_manager.clone(),
                write_lease_manager: self.write_lease_manager.clone(),
                handle: handle.clone(),
                task,
                runtime,
                mailbox_rx,
            };
            let join_handle = tokio::spawn(async move { worker.run().await });
            self.session_manager
                .attach_join_handle(&handle.agent_id, join_handle)
                .map_err(|error| ToolError::invalid_state(error.to_string()))?;
            handles.push(handle);
        }
        Ok(handles)
    }

    async fn send(
        &self,
        parent: SubagentParentContext,
        agent_id: AgentId,
        channel: String,
        payload: Value,
    ) -> std::result::Result<AgentHandle, ToolError> {
        let handle = self
            .session_manager
            .handle(&agent_id)
            .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        self.ensure_parent_can_access(&parent, &handle)?;
        if handle.status.is_terminal() {
            return Ok(handle);
        }
        self.session_manager
            .mailbox(&agent_id)
            .map_err(|error| ToolError::invalid_state(error.to_string()))?
            .send(channel.clone(), payload.clone())
            .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        self.append_agent_envelope(
            &parent,
            &handle,
            AgentEnvelopeKind::Message { channel, payload },
        )
        .await
        .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        Ok(handle)
    }

    async fn wait(
        &self,
        parent: SubagentParentContext,
        request: AgentWaitRequest,
    ) -> std::result::Result<AgentWaitResponse, ToolError> {
        for agent_id in &request.agent_ids {
            let handle = self
                .session_manager
                .handle(agent_id)
                .map_err(|error| ToolError::invalid_state(error.to_string()))?;
            self.ensure_parent_can_access(&parent, &handle)?;
        }
        self.session_manager
            .wait(request)
            .await
            .map_err(|error| ToolError::invalid_state(error.to_string()))
    }

    async fn list(
        &self,
        parent: SubagentParentContext,
    ) -> std::result::Result<Vec<AgentHandle>, ToolError> {
        Ok(self
            .session_manager
            .list()
            .into_iter()
            .filter(|handle| handle.parent_agent_id == parent.parent_agent_id)
            .collect())
    }

    async fn cancel(
        &self,
        parent: SubagentParentContext,
        agent_id: AgentId,
        reason: Option<String>,
    ) -> std::result::Result<AgentHandle, ToolError> {
        let handle = self
            .session_manager
            .handle(&agent_id)
            .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        self.ensure_parent_can_access(&parent, &handle)?;
        if handle.status.is_terminal() {
            return Ok(handle);
        }
        let claimed_files = self.write_lease_manager.claimed_paths(&agent_id);
        let (handle, result) = self
            .session_manager
            .cancel(&agent_id, reason.clone(), claimed_files)
            .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        self.write_lease_manager.release(&agent_id);
        self.append_agent_envelope(
            &parent,
            &handle,
            AgentEnvelopeKind::StatusChanged {
                status: AgentStatus::Cancelled,
            },
        )
        .await
        .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        self.append_agent_envelope(
            &parent,
            &handle,
            AgentEnvelopeKind::Cancelled {
                reason: reason.clone(),
            },
        )
        .await
        .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        self.append_parent_event(
            &parent,
            RunEventKind::TaskCompleted {
                task_id: handle.task_id.clone(),
                agent_id: handle.agent_id.clone(),
                status: AgentStatus::Cancelled,
            },
        )
        .await
        .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        self.append_parent_event(
            &parent,
            RunEventKind::SubagentStop {
                handle: handle.clone(),
                result: Some(result),
                error: reason,
            },
        )
        .await
        .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        Ok(handle)
    }
}

struct ChildAgentWorker {
    parent: SubagentParentContext,
    store: Arc<dyn RunStore>,
    session_manager: AgentSessionManager,
    write_lease_manager: Arc<WriteLeaseManager>,
    handle: AgentHandle,
    task: AgentTaskSpec,
    runtime: AgentRuntime,
    mailbox_rx: AgentMailboxReceiver,
}

impl ChildAgentWorker {
    async fn run(mut self) {
        let handle = match self
            .session_manager
            .update_status(&self.handle.agent_id, AgentStatus::Running)
        {
            Ok(handle) => handle,
            Err(_) => return,
        };
        self.handle = handle.clone();
        let _ = self
            .append_parent_event(RunEventKind::SubagentStart {
                handle: handle.clone(),
                task: self.task.clone(),
            })
            .await;
        let _ = self
            .append_agent_envelope(AgentEnvelopeKind::StatusChanged {
                status: AgentStatus::Running,
            })
            .await;
        let _ = self
            .append_agent_envelope(AgentEnvelopeKind::Started {
                task: self.task.clone(),
            })
            .await;

        if let Some(steer) = self.task.steer.clone() {
            if let Err(error) = self
                .runtime
                .steer(
                    steer,
                    Some(format!("subagent:{}:initial", self.task.task_id)),
                )
                .await
            {
                self.fail(error.to_string()).await;
                return;
            }
        }

        let mut next_prompt = Some(self.task.prompt.clone());
        while let Some(prompt) = next_prompt.take() {
            let outcome = match self.run_prompt(prompt).await {
                Ok(outcome) => outcome,
                Err(error) => {
                    self.fail(error.to_string()).await;
                    return;
                }
            };

            match self.consume_mailbox().await {
                MailboxOutcome::Continue => {
                    next_prompt = Some(
                        "Continue the assigned task using the latest parent steering. Report only new progress."
                            .to_string(),
                    );
                }
                MailboxOutcome::Cancel(reason) => {
                    self.finish_cancel(reason).await;
                    return;
                }
                MailboxOutcome::Finish => {
                    self.finish_success(outcome.assistant_text).await;
                    return;
                }
            }
        }
    }

    async fn run_prompt(&mut self, prompt: String) -> Result<crate::RunTurnOutcome> {
        match self.task.timeout_seconds {
            Some(timeout_seconds) => tokio::time::timeout(
                std::time::Duration::from_secs(timeout_seconds),
                self.runtime.run_user_prompt(prompt),
            )
            .await
            .map_err(|_| RuntimeError::invalid_state("subagent timed out"))?,
            None => self.runtime.run_user_prompt(prompt).await,
        }
    }

    async fn consume_mailbox(&mut self) -> MailboxOutcome {
        let mut continue_requested = false;
        while let Ok(message) = self.mailbox_rx.try_recv() {
            match message {
                AgentControlMessage::Message { channel, payload } => {
                    if channel == "steer" {
                        if let Some(message) = extract_steering_text(&payload) {
                            if self
                                .runtime
                                .steer(
                                    message,
                                    Some(format!("subagent:{}:{channel}", self.task.task_id)),
                                )
                                .await
                                .is_ok()
                            {
                                continue_requested = true;
                            } else {
                                return MailboxOutcome::Cancel(Some(
                                    "failed to apply steering".to_string(),
                                ));
                            }
                        }
                    }
                }
                AgentControlMessage::Cancel { reason } => return MailboxOutcome::Cancel(reason),
            }
        }
        if continue_requested {
            MailboxOutcome::Continue
        } else {
            MailboxOutcome::Finish
        }
    }

    async fn finish_success(&mut self, assistant_text: String) {
        let result = normalize_child_result(
            &self.handle.agent_id,
            &self.task,
            &assistant_text,
            self.write_lease_manager
                .claimed_paths(&self.handle.agent_id),
        );
        let Ok(handle) = self.session_manager.finish(
            &self.handle.agent_id,
            result.status.clone(),
            Some(result.clone()),
            None,
        ) else {
            return;
        };
        let _ = self
            .append_agent_envelope(AgentEnvelopeKind::StatusChanged {
                status: result.status.clone(),
            })
            .await;
        for artifact in &result.artifacts {
            let _ = self
                .append_agent_envelope(AgentEnvelopeKind::Artifact {
                    artifact: artifact.clone(),
                })
                .await;
        }
        let _ = self
            .append_agent_envelope(AgentEnvelopeKind::Result {
                result: result.clone(),
            })
            .await;
        let _ = self
            .append_parent_event(RunEventKind::TaskCompleted {
                task_id: self.task.task_id.clone(),
                agent_id: handle.agent_id.clone(),
                status: result.status.clone(),
            })
            .await;
        let _ = self
            .append_parent_event(RunEventKind::SubagentStop {
                handle,
                result: Some(result),
                error: None,
            })
            .await;
        self.write_lease_manager.release(&self.handle.agent_id);
        let _ = self
            .runtime
            .end_session(Some("completed".to_string()))
            .await;
    }

    async fn finish_cancel(&mut self, reason: Option<String>) {
        let result = AgentResultEnvelope {
            agent_id: self.handle.agent_id.clone(),
            task_id: self.task.task_id.clone(),
            status: AgentStatus::Cancelled,
            summary: reason
                .clone()
                .unwrap_or_else(|| "child agent cancelled".to_string()),
            text: String::new(),
            artifacts: Vec::new(),
            claimed_files: self
                .write_lease_manager
                .claimed_paths(&self.handle.agent_id),
            structured_payload: None,
        };
        let Ok(handle) = self.session_manager.finish(
            &self.handle.agent_id,
            AgentStatus::Cancelled,
            Some(result.clone()),
            reason.clone(),
        ) else {
            return;
        };
        let _ = self
            .append_agent_envelope(AgentEnvelopeKind::StatusChanged {
                status: AgentStatus::Cancelled,
            })
            .await;
        let _ = self
            .append_agent_envelope(AgentEnvelopeKind::Cancelled {
                reason: reason.clone(),
            })
            .await;
        let _ = self
            .append_parent_event(RunEventKind::TaskCompleted {
                task_id: self.task.task_id.clone(),
                agent_id: handle.agent_id.clone(),
                status: AgentStatus::Cancelled,
            })
            .await;
        let _ = self
            .append_parent_event(RunEventKind::SubagentStop {
                handle,
                result: Some(result),
                error: reason.clone(),
            })
            .await;
        self.write_lease_manager.release(&self.handle.agent_id);
        let _ = self.runtime.end_session(reason).await;
    }

    async fn fail(&mut self, error: String) {
        let result = AgentResultEnvelope {
            agent_id: self.handle.agent_id.clone(),
            task_id: self.task.task_id.clone(),
            status: AgentStatus::Failed,
            summary: summarize_output(&error),
            text: error.clone(),
            artifacts: Vec::new(),
            claimed_files: self
                .write_lease_manager
                .claimed_paths(&self.handle.agent_id),
            structured_payload: None,
        };
        let Ok(handle) = self.session_manager.finish(
            &self.handle.agent_id,
            AgentStatus::Failed,
            Some(result.clone()),
            Some(error.clone()),
        ) else {
            return;
        };
        let _ = self
            .append_agent_envelope(AgentEnvelopeKind::StatusChanged {
                status: AgentStatus::Failed,
            })
            .await;
        let _ = self
            .append_agent_envelope(AgentEnvelopeKind::Failed {
                error: error.clone(),
            })
            .await;
        let _ = self
            .append_parent_event(RunEventKind::TaskCompleted {
                task_id: self.task.task_id.clone(),
                agent_id: handle.agent_id.clone(),
                status: AgentStatus::Failed,
            })
            .await;
        let _ = self
            .append_parent_event(RunEventKind::SubagentStop {
                handle,
                result: Some(result),
                error: Some(error),
            })
            .await;
        self.write_lease_manager.release(&self.handle.agent_id);
        let _ = self.runtime.end_session(Some("failed".to_string())).await;
    }

    async fn append_parent_event(&self, event: RunEventKind) -> Result<()> {
        let Some(run_id) = self.parent.run_id.clone() else {
            return Ok(());
        };
        let Some(session_id) = self.parent.session_id.clone() else {
            return Ok(());
        };
        self.store
            .append(RunEventEnvelope::new(
                run_id,
                session_id,
                self.parent.turn_id.clone(),
                None,
                event,
            ))
            .await
            .map_err(RuntimeError::from)
    }

    async fn append_agent_envelope(&self, kind: AgentEnvelopeKind) -> Result<()> {
        self.append_parent_event(RunEventKind::AgentEnvelope {
            envelope: AgentEnvelope::new(
                self.handle.agent_id.clone(),
                self.handle.parent_agent_id.clone(),
                self.handle.run_id.clone(),
                self.handle.session_id.clone(),
                kind,
            ),
        })
        .await
    }
}

enum MailboxOutcome {
    Continue,
    Cancel(Option<String>),
    Finish,
}

#[derive(Deserialize)]
struct StructuredChildPayload {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    artifacts: Vec<StructuredChildArtifact>,
    #[serde(default)]
    structured_payload: Option<Value>,
}

#[derive(Deserialize)]
struct StructuredChildArtifact {
    kind: String,
    uri: String,
    #[serde(default)]
    label: Option<String>,
}

fn normalize_child_result(
    agent_id: &AgentId,
    task: &AgentTaskSpec,
    assistant_text: &str,
    claimed_files: Vec<String>,
) -> AgentResultEnvelope {
    if let Ok(payload) = serde_json::from_str::<StructuredChildPayload>(assistant_text.trim()) {
        return AgentResultEnvelope {
            agent_id: agent_id.clone(),
            task_id: task.task_id.clone(),
            // A child may report a richer terminal outcome, but the successful
            // return path must never persist a non-terminal status or waits can
            // hang forever on an agent that already exited.
            status: normalize_terminal_result_status(payload.status.as_deref()),
            summary: payload
                .summary
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| summarize_output(assistant_text)),
            text: payload.text.unwrap_or_else(|| assistant_text.to_string()),
            artifacts: payload
                .artifacts
                .into_iter()
                .map(|artifact| AgentArtifact {
                    kind: artifact.kind,
                    uri: artifact.uri,
                    label: artifact.label,
                    metadata: None,
                })
                .collect(),
            claimed_files,
            structured_payload: payload.structured_payload,
        };
    }

    AgentResultEnvelope {
        agent_id: agent_id.clone(),
        task_id: task.task_id.clone(),
        status: AgentStatus::Completed,
        summary: summarize_output(assistant_text),
        text: assistant_text.to_string(),
        artifacts: extract_artifacts(assistant_text),
        claimed_files,
        structured_payload: None,
    }
}

fn normalize_terminal_result_status(value: Option<&str>) -> AgentStatus {
    let status = value.map(parse_status).unwrap_or(AgentStatus::Completed);
    if status.is_terminal() {
        status
    } else {
        AgentStatus::Completed
    }
}

fn parse_status(value: &str) -> AgentStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "queued" => AgentStatus::Queued,
        "running" => AgentStatus::Running,
        "waiting_approval" => AgentStatus::WaitingApproval,
        "waiting_message" => AgentStatus::WaitingMessage,
        "failed" => AgentStatus::Failed,
        "cancelled" => AgentStatus::Cancelled,
        _ => AgentStatus::Completed,
    }
}

fn summarize_output(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        "child agent completed without output".to_string()
    } else {
        collapsed.chars().take(96).collect()
    }
}

fn extract_artifacts(text: &str) -> Vec<AgentArtifact> {
    text.split_whitespace()
        .filter(|token| token.starts_with("http://") || token.starts_with("https://"))
        .map(|token| AgentArtifact {
            kind: "url".to_string(),
            uri: token
                .trim_matches(|c| c == ')' || c == ']' || c == ',')
                .to_string(),
            label: None,
            metadata: None,
        })
        .collect()
}

fn extract_steering_text(payload: &Value) -> Option<String> {
    payload
        .as_str()
        .map(ToString::to_string)
        .or_else(|| {
            payload
                .get("message")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .or_else(|| {
            if payload.is_null() {
                None
            } else {
                Some(payload.to_string())
            }
        })
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::RuntimeSubagentExecutor;
    use crate::Result;
    use crate::{
        AlwaysAllowToolApprovalHandler, CompactionConfig, HookRunner, LoopDetectionConfig,
        ModelBackend, NoopConversationCompactor, NoopToolApprovalPolicy,
    };
    use async_trait::async_trait;
    use futures::{StreamExt, stream, stream::BoxStream};
    use skills::SkillCatalog;
    use std::sync::{Arc, Mutex};
    use store::{InMemoryRunStore, RunStore};
    use tokio::sync::Notify;
    use tools::{
        ReadTool, SubagentExecutor, SubagentParentContext, ToolExecutionContext, ToolRegistry,
    };
    use types::{
        AgentEnvelopeKind, AgentStatus, AgentTaskSpec, AgentWaitMode, AgentWaitRequest,
        MessageRole, ModelEvent, ModelRequest, RunEventKind, ToolName,
    };

    #[derive(Clone, Default)]
    struct ImmediateBackend {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    #[async_trait]
    impl ModelBackend for ImmediateBackend {
        async fn stream_turn(
            &self,
            request: ModelRequest,
        ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
            self.requests.lock().unwrap().push(request);
            Ok(stream::iter(vec![
                Ok(ModelEvent::TextDelta {
                    delta: "child ok https://example.com/report".to_string(),
                }),
                Ok(ModelEvent::ResponseComplete {
                    stop_reason: Some("stop".to_string()),
                    message_id: None,
                    continuation: None,
                    reasoning: Vec::new(),
                }),
            ])
            .boxed())
        }
    }

    #[derive(Clone)]
    struct BlockingBackend {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
        started: Arc<Notify>,
        release: Arc<Notify>,
        first_user_request_pending: Arc<Mutex<bool>>,
    }

    #[async_trait]
    impl ModelBackend for BlockingBackend {
        async fn stream_turn(
            &self,
            request: ModelRequest,
        ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
            self.requests.lock().unwrap().push(request.clone());
            if request
                .messages
                .iter()
                .any(|message| message.role == MessageRole::User)
            {
                let should_wait = {
                    let mut pending = self.first_user_request_pending.lock().unwrap();
                    if *pending {
                        *pending = false;
                        true
                    } else {
                        false
                    }
                };
                if should_wait {
                    self.started.notify_waiters();
                    self.release.notified().await;
                }
            }
            let text = request
                .messages
                .iter()
                .find(|message| message.role == MessageRole::System)
                .map(|message| format!("child ok {}", message.text_content()))
                .unwrap_or_else(|| "child ok".to_string());
            Ok(stream::iter(vec![
                Ok(ModelEvent::TextDelta { delta: text }),
                Ok(ModelEvent::ResponseComplete {
                    stop_reason: Some("stop".to_string()),
                    message_id: None,
                    continuation: None,
                    reasoning: Vec::new(),
                }),
            ])
            .boxed())
        }
    }

    fn make_executor(
        backend: Arc<dyn ModelBackend>,
        store: Arc<dyn RunStore>,
    ) -> RuntimeSubagentExecutor {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = ToolRegistry::new();
        registry.register(ReadTool::new());
        RuntimeSubagentExecutor::new(
            backend,
            Arc::new(HookRunner::default()),
            store,
            registry,
            ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                ..Default::default()
            },
            Arc::new(AlwaysAllowToolApprovalHandler),
            Arc::new(NoopToolApprovalPolicy),
            Arc::new(NoopConversationCompactor),
            CompactionConfig::default(),
            LoopDetectionConfig::default(),
            vec!["static instruction".to_string()],
            Vec::new(),
            SkillCatalog::default(),
        )
    }

    #[tokio::test]
    async fn runtime_subagent_executor_spawns_batch_and_waits_all() {
        let backend = Arc::new(ImmediateBackend::default());
        let store = Arc::new(InMemoryRunStore::new());
        let executor = make_executor(backend, store);

        let handles = executor
            .spawn(
                SubagentParentContext {
                    run_id: Some("run_parent".into()),
                    session_id: Some("session_parent".into()),
                    turn_id: Some("turn_parent".into()),
                    parent_agent_id: Some("agent_parent".into()),
                },
                vec![
                    AgentTaskSpec {
                        task_id: "inspect".to_string(),
                        role: "explorer".to_string(),
                        prompt: "inspect".to_string(),
                        steer: None,
                        allowed_tools: vec![ToolName::from("read")],
                        requested_write_set: Vec::new(),
                        dependency_ids: Vec::new(),
                        timeout_seconds: None,
                    },
                    AgentTaskSpec {
                        task_id: "review".to_string(),
                        role: "reviewer".to_string(),
                        prompt: "review".to_string(),
                        steer: None,
                        allowed_tools: vec![ToolName::from("read")],
                        requested_write_set: Vec::new(),
                        dependency_ids: Vec::new(),
                        timeout_seconds: None,
                    },
                ],
            )
            .await
            .unwrap();

        let wait = executor
            .wait(
                SubagentParentContext::default(),
                AgentWaitRequest {
                    agent_ids: handles
                        .iter()
                        .map(|handle| handle.agent_id.clone())
                        .collect(),
                    mode: AgentWaitMode::All,
                },
            )
            .await
            .unwrap();

        assert_eq!(wait.completed.len(), 2);
        assert_eq!(wait.results.len(), 2);
        assert!(
            wait.results
                .iter()
                .all(|result| result.status == AgentStatus::Completed)
        );
    }

    #[tokio::test]
    async fn runtime_subagent_executor_applies_steering_and_emits_lifecycle_events() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let backend = Arc::new(BlockingBackend {
            requests: Arc::new(Mutex::new(Vec::new())),
            started: started.clone(),
            release: release.clone(),
            first_user_request_pending: Arc::new(Mutex::new(true)),
        });
        let store = Arc::new(InMemoryRunStore::new());
        let executor = make_executor(backend.clone(), store.clone());
        let parent = SubagentParentContext {
            run_id: Some("run_parent".into()),
            session_id: Some("session_parent".into()),
            turn_id: Some("turn_parent".into()),
            parent_agent_id: Some("agent_parent".into()),
        };

        let handles = executor
            .spawn(
                parent.clone(),
                vec![AgentTaskSpec {
                    task_id: "inspect".to_string(),
                    role: "explorer".to_string(),
                    prompt: "inspect workspace".to_string(),
                    steer: None,
                    allowed_tools: vec![ToolName::from("read")],
                    requested_write_set: Vec::new(),
                    dependency_ids: Vec::new(),
                    timeout_seconds: None,
                }],
            )
            .await
            .unwrap();

        started.notified().await;
        executor
            .send(
                parent.clone(),
                handles[0].agent_id.clone(),
                "steer".to_string(),
                serde_json::json!({"message":"focus tests"}),
            )
            .await
            .unwrap();
        release.notify_waiters();

        let wait = executor
            .wait(
                parent.clone(),
                AgentWaitRequest {
                    agent_ids: vec![handles[0].agent_id.clone()],
                    mode: AgentWaitMode::All,
                },
            )
            .await
            .unwrap();
        assert_eq!(wait.results[0].status, AgentStatus::Completed);
        assert!(backend.requests.lock().unwrap().iter().any(|request| {
            request
                .messages
                .iter()
                .any(|message| message.text_content().contains("focus tests"))
        }));

        let events = store.events(&"run_parent".into()).await.unwrap();
        assert!(
            events
                .iter()
                .any(|event| matches!(event.event, RunEventKind::TaskCreated { .. }))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event.event, RunEventKind::SubagentStart { .. }))
        );
        assert!(events.iter().any(|event| matches!(
            &event.event,
            RunEventKind::AgentEnvelope {
                envelope: types::AgentEnvelope {
                    kind: AgentEnvelopeKind::Message { .. },
                    ..
                },
            }
        )));
        assert!(events.iter().any(|event| matches!(
            &event.event,
            RunEventKind::AgentEnvelope {
                envelope: types::AgentEnvelope {
                    kind: AgentEnvelopeKind::Result { .. },
                    ..
                },
            }
        )));
        assert!(
            events
                .iter()
                .any(|event| matches!(event.event, RunEventKind::SubagentStop { .. }))
        );
    }

    #[tokio::test]
    async fn runtime_subagent_executor_cancels_running_agent() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let backend = Arc::new(BlockingBackend {
            requests: Arc::new(Mutex::new(Vec::new())),
            started: started.clone(),
            release,
            first_user_request_pending: Arc::new(Mutex::new(true)),
        });
        let store = Arc::new(InMemoryRunStore::new());
        let executor = make_executor(backend, store.clone());
        let parent = SubagentParentContext {
            run_id: Some("run_parent".into()),
            session_id: Some("session_parent".into()),
            turn_id: Some("turn_parent".into()),
            parent_agent_id: Some("agent_parent".into()),
        };

        let handles = executor
            .spawn(
                parent.clone(),
                vec![AgentTaskSpec {
                    task_id: "cancel_me".to_string(),
                    role: "explorer".to_string(),
                    prompt: "inspect".to_string(),
                    steer: None,
                    allowed_tools: vec![ToolName::from("read")],
                    requested_write_set: Vec::new(),
                    dependency_ids: Vec::new(),
                    timeout_seconds: None,
                }],
            )
            .await
            .unwrap();
        started.notified().await;
        executor
            .cancel(
                parent.clone(),
                handles[0].agent_id.clone(),
                Some("stop".to_string()),
            )
            .await
            .unwrap();

        let wait = executor
            .wait(
                parent,
                AgentWaitRequest {
                    agent_ids: vec![handles[0].agent_id.clone()],
                    mode: AgentWaitMode::All,
                },
            )
            .await
            .unwrap();
        assert_eq!(wait.results[0].status, AgentStatus::Cancelled);
    }

    #[tokio::test]
    async fn runtime_subagent_executor_rejects_conflicting_write_leases() {
        let backend = Arc::new(ImmediateBackend::default());
        let store = Arc::new(InMemoryRunStore::new());
        let executor = make_executor(backend, store);

        executor
            .spawn(
                SubagentParentContext::default(),
                vec![AgentTaskSpec {
                    task_id: "one".to_string(),
                    role: "writer".to_string(),
                    prompt: "write".to_string(),
                    steer: None,
                    allowed_tools: vec![ToolName::from("read")],
                    requested_write_set: vec!["src".to_string()],
                    dependency_ids: Vec::new(),
                    timeout_seconds: None,
                }],
            )
            .await
            .unwrap();

        let error = executor
            .spawn(
                SubagentParentContext::default(),
                vec![AgentTaskSpec {
                    task_id: "two".to_string(),
                    role: "writer".to_string(),
                    prompt: "write".to_string(),
                    steer: None,
                    allowed_tools: vec![ToolName::from("read")],
                    requested_write_set: vec!["src/lib.rs".to_string()],
                    dependency_ids: Vec::new(),
                    timeout_seconds: None,
                }],
            )
            .await
            .expect_err("conflicting lease must fail");
        assert!(error.to_string().contains("write lease conflict"));
    }

    #[tokio::test]
    async fn runtime_subagent_executor_rejects_cross_parent_control_requests() {
        let backend = Arc::new(BlockingBackend {
            requests: Arc::new(Mutex::new(Vec::new())),
            started: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
            first_user_request_pending: Arc::new(Mutex::new(false)),
        });
        let store = Arc::new(InMemoryRunStore::new());
        let executor = make_executor(backend, store);
        let owner = SubagentParentContext {
            run_id: Some("run_parent".into()),
            session_id: Some("session_parent".into()),
            turn_id: Some("turn_parent".into()),
            parent_agent_id: Some("agent_owner".into()),
        };
        let intruder = SubagentParentContext {
            parent_agent_id: Some("agent_intruder".into()),
            ..owner.clone()
        };

        let handles = executor
            .spawn(
                owner.clone(),
                vec![AgentTaskSpec {
                    task_id: "inspect".to_string(),
                    role: "explorer".to_string(),
                    prompt: "inspect".to_string(),
                    steer: None,
                    allowed_tools: vec![ToolName::from("read")],
                    requested_write_set: Vec::new(),
                    dependency_ids: Vec::new(),
                    timeout_seconds: None,
                }],
            )
            .await
            .unwrap();

        let send_error = executor
            .send(
                intruder.clone(),
                handles[0].agent_id.clone(),
                "steer".to_string(),
                serde_json::json!({"message":"nope"}),
            )
            .await
            .expect_err("foreign parent must not send");
        assert!(send_error.to_string().contains("cannot access child agent"));

        let wait_error = executor
            .wait(
                intruder.clone(),
                AgentWaitRequest {
                    agent_ids: vec![handles[0].agent_id.clone()],
                    mode: AgentWaitMode::All,
                },
            )
            .await
            .expect_err("foreign parent must not wait");
        assert!(wait_error.to_string().contains("cannot access child agent"));

        let cancel_error = executor
            .cancel(
                intruder,
                handles[0].agent_id.clone(),
                Some("stop".to_string()),
            )
            .await
            .expect_err("foreign parent must not cancel");
        assert!(
            cancel_error
                .to_string()
                .contains("cannot access child agent")
        );
    }

    #[tokio::test]
    async fn runtime_subagent_executor_root_list_only_shows_root_owned_children() {
        let backend = Arc::new(ImmediateBackend::default());
        let store = Arc::new(InMemoryRunStore::new());
        let executor = make_executor(backend, store);
        executor
            .spawn(
                SubagentParentContext::default(),
                vec![AgentTaskSpec {
                    task_id: "root".to_string(),
                    role: "explorer".to_string(),
                    prompt: "inspect".to_string(),
                    steer: None,
                    allowed_tools: vec![ToolName::from("read")],
                    requested_write_set: Vec::new(),
                    dependency_ids: Vec::new(),
                    timeout_seconds: None,
                }],
            )
            .await
            .unwrap();
        executor
            .spawn(
                SubagentParentContext {
                    parent_agent_id: Some("agent_parent".into()),
                    ..SubagentParentContext::default()
                },
                vec![AgentTaskSpec {
                    task_id: "nested".to_string(),
                    role: "explorer".to_string(),
                    prompt: "inspect".to_string(),
                    steer: None,
                    allowed_tools: vec![ToolName::from("read")],
                    requested_write_set: Vec::new(),
                    dependency_ids: Vec::new(),
                    timeout_seconds: None,
                }],
            )
            .await
            .unwrap();

        let root_handles = executor
            .list(SubagentParentContext::default())
            .await
            .unwrap();
        assert_eq!(root_handles.len(), 1);
        assert_eq!(root_handles[0].task_id, "root");
    }

    #[test]
    fn normalize_child_result_coerces_non_terminal_payload_status_to_completed() {
        let result = normalize_child_result(
            &types::AgentId::from("agent_1"),
            &AgentTaskSpec {
                task_id: "task_1".to_string(),
                role: "explorer".to_string(),
                prompt: "inspect".to_string(),
                steer: None,
                allowed_tools: Vec::new(),
                requested_write_set: Vec::new(),
                dependency_ids: Vec::new(),
                timeout_seconds: None,
            },
            r#"{"status":"running","summary":"still going","text":"done"}"#,
            Vec::new(),
        );

        assert_eq!(result.status, AgentStatus::Completed);
        assert_eq!(result.summary, "still going");
    }
}
