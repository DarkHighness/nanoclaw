use crate::agent_mailbox::{AgentControlMessage, AgentMailboxReceiver, agent_mailbox_channel};
use crate::{
    AgentRuntime, AgentRuntimeBuilder, AgentSessionManager, CompactionConfig,
    ConversationCompactor, HookRunner, LoopDetectionConfig, ModelBackend, Result, RuntimeError,
    ToolApprovalHandler, ToolApprovalPolicy, WriteLeaseManager,
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
};
use types::{
    AgentArtifact, AgentEnvelope, AgentEnvelopeKind, AgentHandle, AgentId, AgentResultEnvelope,
    AgentStatus, AgentTaskSpec, AgentWaitRequest, AgentWaitResponse, HookRegistration,
    RunEventEnvelope, RunEventKind, ToolName,
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
    write_lease_manager: WriteLeaseManager,
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
            write_lease_manager: WriteLeaseManager::new(),
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
}

#[async_trait]
impl SubagentExecutor for RuntimeSubagentExecutor {
    async fn spawn(
        &self,
        parent: SubagentParentContext,
        tasks: Vec<AgentTaskSpec>,
    ) -> std::result::Result<Vec<AgentHandle>, ToolError> {
        let mut created = Vec::new();
        for task in tasks {
            let agent_id = AgentId::new();
            let requested_paths = task
                .requested_write_set
                .iter()
                .map(PathBuf::from)
                .collect::<Vec<_>>();
            self.write_lease_manager
                .claim(&agent_id, &requested_paths)
                .map_err(|conflict| {
                    ToolError::invalid_state(format!(
                        "write lease conflict on {} owned by {} via {}",
                        conflict.requested, conflict.owner, conflict.owner_path
                    ))
                })?;
            let (tool_registry, resolved_tools) = self
                .resolve_child_tools(&task.allowed_tools)
                .map_err(|error| ToolError::invalid_state(error.to_string()))?;
            let runtime = AgentRuntimeBuilder::new(self.backend.clone(), self.store.clone())
                .hook_runner(self.hook_runner.clone())
                .tool_registry(tool_registry)
                .tool_context(self.tool_context.clone())
                .tool_approval_handler(self.tool_approval_handler.clone())
                .tool_approval_policy(self.tool_approval_policy.clone())
                .conversation_compactor(self.conversation_compactor.clone())
                .compaction_config(self.compaction_config.clone())
                .loop_detection_config(self.loop_detection_config.clone())
                .instructions(self.instructions.clone())
                .hooks(self.hooks.clone())
                .skill_catalog(self.skill_catalog.clone())
                .build();
            let handle = AgentHandle {
                agent_id: agent_id.clone(),
                parent_agent_id: parent.parent_agent_id.clone(),
                run_id: runtime.run_id(),
                session_id: runtime.session_id(),
                task_id: task.task_id.clone(),
                role: task.role.clone(),
                status: AgentStatus::Queued,
            };
            let effective_task = AgentTaskSpec {
                allowed_tools: resolved_tools,
                ..task.clone()
            };
            let (mailbox, mailbox_rx) = agent_mailbox_channel();
            self.session_manager
                .insert(handle.clone(), effective_task.clone(), mailbox);
            self.append_parent_event(
                &parent,
                RunEventKind::TaskCreated {
                    task: effective_task.clone(),
                    parent_agent_id: parent.parent_agent_id.clone(),
                },
            )
            .await
            .map_err(|error| ToolError::invalid_state(error.to_string()))?;
            let worker = ChildAgentWorker {
                parent: parent.clone(),
                store: self.store.clone(),
                session_manager: self.session_manager.clone(),
                write_lease_manager: self.write_lease_manager.clone(),
                handle: handle.clone(),
                task: effective_task,
                runtime,
                mailbox_rx,
            };
            let join_handle = tokio::spawn(async move { worker.run().await });
            self.session_manager
                .attach_join_handle(&handle.agent_id, join_handle)
                .map_err(|error| ToolError::invalid_state(error.to_string()))?;
            created.push(handle);
        }
        Ok(created)
    }

    async fn send(
        &self,
        parent: SubagentParentContext,
        agent_id: AgentId,
        channel: String,
        payload: Value,
    ) -> std::result::Result<AgentHandle, ToolError> {
        self.session_manager
            .mailbox(&agent_id)
            .map_err(|error| ToolError::invalid_state(error.to_string()))?
            .send(channel.clone(), payload.clone())
            .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        let handle = self
            .session_manager
            .handle(&agent_id)
            .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        self.append_parent_event(
            &parent,
            RunEventKind::AgentEnvelope {
                envelope: AgentEnvelope::new(
                    handle.agent_id.clone(),
                    handle.parent_agent_id.clone(),
                    handle.run_id.clone(),
                    handle.session_id.clone(),
                    AgentEnvelopeKind::Message { channel, payload },
                ),
            },
        )
        .await
        .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        Ok(handle)
    }

    async fn wait(
        &self,
        _parent: SubagentParentContext,
        request: AgentWaitRequest,
    ) -> std::result::Result<AgentWaitResponse, ToolError> {
        self.session_manager
            .wait(request)
            .await
            .map_err(|error| ToolError::invalid_state(error.to_string()))
    }

    async fn list(
        &self,
        _parent: SubagentParentContext,
    ) -> std::result::Result<Vec<AgentHandle>, ToolError> {
        Ok(self.session_manager.list())
    }

    async fn cancel(
        &self,
        _parent: SubagentParentContext,
        agent_id: AgentId,
        reason: Option<String>,
    ) -> std::result::Result<AgentHandle, ToolError> {
        self.session_manager
            .mailbox(&agent_id)
            .map_err(|error| ToolError::invalid_state(error.to_string()))?
            .cancel(reason.clone())
            .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        let handle = self
            .session_manager
            .cancel(
                &agent_id,
                reason,
                self.write_lease_manager.claimed_paths(&agent_id),
            )
            .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        self.write_lease_manager.release(&agent_id);
        Ok(handle.0)
    }
}

struct ChildAgentWorker {
    parent: SubagentParentContext,
    store: Arc<dyn RunStore>,
    session_manager: AgentSessionManager,
    write_lease_manager: WriteLeaseManager,
    handle: AgentHandle,
    task: AgentTaskSpec,
    runtime: AgentRuntime,
    mailbox_rx: AgentMailboxReceiver,
}

impl ChildAgentWorker {
    async fn run(mut self) {
        if let Ok(handle) = self
            .session_manager
            .update_status(&self.handle.agent_id, AgentStatus::Running)
        {
            let _ = self
                .append_parent_event(RunEventKind::SubagentStart {
                    handle,
                    task: self.task.clone(),
                })
                .await;
        }
        if let Some(steer) = self.task.steer.clone() {
            let _ = self
                .runtime
                .steer(
                    steer,
                    Some(format!("subagent:{}:initial", self.task.task_id)),
                )
                .await;
        }
        let mut next_prompt = Some(self.task.prompt.clone());
        while let Some(prompt) = next_prompt.take() {
            let outcome = match self.runtime.run_user_prompt(prompt).await {
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
                    self.cancel(reason).await;
                    return;
                }
                MailboxOutcome::Finish => {
                    let result = normalize_child_result(
                        &self.handle.agent_id,
                        &self.task,
                        &outcome.assistant_text,
                        self.write_lease_manager
                            .claimed_paths(&self.handle.agent_id),
                    );
                    if let Ok(handle) = self.session_manager.finish(
                        &self.handle.agent_id,
                        result.status.clone(),
                        Some(result.clone()),
                        None,
                    ) {
                        let _ = self
                            .append_parent_event(RunEventKind::SubagentStop {
                                handle,
                                result: Some(result),
                                error: None,
                            })
                            .await;
                    }
                    self.write_lease_manager.release(&self.handle.agent_id);
                    return;
                }
            }
        }
    }

    async fn consume_mailbox(&mut self) -> MailboxOutcome {
        let mut continue_requested = false;
        while let Ok(message) = self.mailbox_rx.try_recv() {
            match message {
                AgentControlMessage::Message { channel, payload } => {
                    if channel == "steer" {
                        if let Some(steering) = extract_steering_text(&payload) {
                            let _ = self
                                .runtime
                                .steer(
                                    steering,
                                    Some(format!("subagent:{}:{channel}", self.task.task_id)),
                                )
                                .await;
                            continue_requested = true;
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

    async fn cancel(&mut self, reason: Option<String>) {
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
        let _ = self.session_manager.finish(
            &self.handle.agent_id,
            AgentStatus::Cancelled,
            Some(result),
            reason,
        );
        self.write_lease_manager.release(&self.handle.agent_id);
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
        let _ = self.session_manager.finish(
            &self.handle.agent_id,
            AgentStatus::Failed,
            Some(result),
            Some(error),
        );
        self.write_lease_manager.release(&self.handle.agent_id);
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
            status: payload
                .status
                .as_deref()
                .map(parse_status)
                .unwrap_or(AgentStatus::Completed),
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
        artifacts: Vec::new(),
        claimed_files,
        structured_payload: None,
    }
}

fn parse_status(value: &str) -> AgentStatus {
    match value.trim().to_ascii_lowercase().as_str() {
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
        AgentWaitMode, AgentWaitRequest, ModelEvent, ModelRequest, RunEventKind, ToolName,
    };

    #[derive(Clone)]
    struct BlockingBackend {
        started: Arc<Notify>,
        release: Arc<Notify>,
        requests: Arc<Mutex<Vec<ModelRequest>>>,
        first_user_request_pending: Arc<Mutex<bool>>,
    }

    #[async_trait]
    impl ModelBackend for BlockingBackend {
        async fn stream_turn(
            &self,
            request: ModelRequest,
        ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
            let should_wait = if request
                .messages
                .iter()
                .any(|message| message.role == types::MessageRole::User)
            {
                let mut pending = self.first_user_request_pending.lock().unwrap();
                let should_wait = *pending;
                *pending = false;
                should_wait
            } else {
                false
            };
            self.requests.lock().unwrap().push(request);
            if should_wait {
                self.started.notify_waiters();
                self.release.notified().await;
            }
            Ok(stream::iter(vec![
                Ok(ModelEvent::TextDelta {
                    delta: "child ok".to_string(),
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
    async fn runtime_subagent_executor_spawns_and_waits() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let backend = Arc::new(BlockingBackend {
            started: started.clone(),
            release: release.clone(),
            requests: Arc::new(Mutex::new(Vec::new())),
            first_user_request_pending: Arc::new(Mutex::new(true)),
        });
        let store = Arc::new(InMemoryRunStore::new());
        let executor = make_executor(backend, store.clone());
        let parent = SubagentParentContext {
            run_id: Some("run_parent".into()),
            session_id: Some("session_parent".into()),
            turn_id: Some("turn_parent".into()),
            parent_agent_id: None,
        };
        let handles = executor
            .spawn(
                parent.clone(),
                vec![types::AgentTaskSpec {
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
                parent,
                AgentWaitRequest {
                    agent_ids: vec![handles[0].agent_id.clone()],
                    mode: AgentWaitMode::All,
                },
            )
            .await
            .unwrap();
        assert_eq!(wait.completed.len(), 1);
        let events = store.events(&"run_parent".into()).await.unwrap();
        assert!(
            events
                .iter()
                .any(|event| matches!(event.event, RunEventKind::SubagentStart { .. }))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event.event, RunEventKind::SubagentStop { .. }))
        );
    }
}
