use crate::agent_mailbox::{AgentControlMessage, AgentMailboxReceiver, agent_mailbox_channel};
use crate::{
    AgentRuntime, AgentRuntimeBuilder, AgentSessionManager, CompactionConfig,
    ConversationCompactor, HookRunner, LoopDetectionConfig, ModelBackend, Result, RuntimeError,
    RuntimeSession, ToolApprovalHandler, ToolApprovalPolicy, WriteLeaseManager,
};
use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use serde::Deserialize;
use serde_json::Value;
use skills::SkillCatalog;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use store::RunStore;
use tools::{
    SubagentExecutor, SubagentParentContext, ToolError, ToolExecutionContext, ToolRegistry,
    resolve_tool_path_against_workspace_root,
};
use types::{
    AgentArtifact, AgentEnvelope, AgentEnvelopeKind, AgentHandle, AgentId, AgentResultEnvelope,
    AgentSessionId, AgentStatus, AgentTaskSpec, AgentWaitRequest, AgentWaitResponse,
    HookRegistration, RunEventEnvelope, RunEventKind, RunId, ToolName,
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
const READY_CHILD_LAUNCH_CONCURRENCY: usize = 4;

#[derive(Clone)]
pub struct SubagentRuntimeProfile {
    pub profile_name: String,
    pub backend: Arc<dyn ModelBackend>,
    pub tool_context: ToolExecutionContext,
    pub conversation_compactor: Arc<dyn ConversationCompactor>,
    pub compaction_config: CompactionConfig,
    pub instructions: Vec<String>,
    pub supports_tool_calls: bool,
}

pub trait SubagentProfileResolver: Send + Sync {
    fn resolve_profile(&self, task: &AgentTaskSpec) -> Result<SubagentRuntimeProfile>;
}

#[derive(Clone)]
pub struct RuntimeSubagentExecutor {
    hook_runner: Arc<HookRunner>,
    store: Arc<dyn RunStore>,
    tool_registry: ToolRegistry,
    tool_context: ToolExecutionContext,
    tool_approval_handler: Arc<dyn ToolApprovalHandler>,
    tool_approval_policy: Arc<dyn ToolApprovalPolicy>,
    loop_detection_config: LoopDetectionConfig,
    hooks: Vec<HookRegistration>,
    skill_catalog: SkillCatalog,
    profile_resolver: Arc<dyn SubagentProfileResolver>,
    session_manager: AgentSessionManager,
    write_lease_manager: Arc<WriteLeaseManager>,
}

struct PlannedChildSpawn {
    handle: AgentHandle,
    task: AgentTaskSpec,
    tool_registry: ToolRegistry,
    requested_files: Vec<PathBuf>,
    dependency_agent_ids: Vec<AgentId>,
    profile: SubagentRuntimeProfile,
}

#[derive(Clone)]
struct DependencyFailure {
    task_id: String,
    status: AgentStatus,
    summary: Option<String>,
}

struct ChildLaunchPlan {
    parent: SubagentParentContext,
    handle: AgentHandle,
    task: AgentTaskSpec,
    tool_registry: ToolRegistry,
    mailbox_rx: AgentMailboxReceiver,
    dependency_agent_ids: Vec<AgentId>,
    profile: SubagentRuntimeProfile,
}

enum DependencyGate {
    Ready,
    Blocked(BTreeSet<AgentId>),
    Failed(Vec<DependencyFailure>),
}

async fn run_bounded_launches<T, F, Fut>(items: Vec<T>, limit: usize, launch: F)
where
    T: Send + 'static,
    F: Fn(T) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    stream::iter(items)
        .for_each_concurrent(limit.max(1), launch)
        .await;
}

impl RuntimeSubagentExecutor {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        hook_runner: Arc<HookRunner>,
        store: Arc<dyn RunStore>,
        tool_registry: ToolRegistry,
        tool_context: ToolExecutionContext,
        tool_approval_handler: Arc<dyn ToolApprovalHandler>,
        tool_approval_policy: Arc<dyn ToolApprovalPolicy>,
        loop_detection_config: LoopDetectionConfig,
        hooks: Vec<HookRegistration>,
        skill_catalog: SkillCatalog,
        profile_resolver: Arc<dyn SubagentProfileResolver>,
    ) -> Self {
        Self {
            hook_runner,
            store,
            tool_registry,
            tool_context,
            tool_approval_handler,
            tool_approval_policy,
            loop_detection_config,
            hooks,
            skill_catalog,
            profile_resolver,
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
        self.append_parent_events(parent, vec![event]).await
    }

    async fn append_parent_events(
        &self,
        parent: &SubagentParentContext,
        events: Vec<RunEventKind>,
    ) -> Result<()> {
        let Some(run_id) = parent.run_id.clone() else {
            return Ok(());
        };
        let Some(agent_session_id) = parent.agent_session_id.clone() else {
            return Ok(());
        };
        self.store
            .append_batch(
                events
                    .into_iter()
                    .map(|event| {
                        RunEventEnvelope::new(
                            run_id.clone(),
                            agent_session_id.clone(),
                            parent.turn_id.clone(),
                            None,
                            event,
                        )
                    })
                    .collect(),
            )
            .await
            .map_err(RuntimeError::from)
    }

    async fn append_agent_envelope(
        &self,
        parent: &SubagentParentContext,
        handle: &AgentHandle,
        kind: AgentEnvelopeKind,
    ) -> Result<()> {
        self.append_agent_envelopes(parent, handle, vec![kind])
            .await
    }

    async fn append_agent_envelopes(
        &self,
        parent: &SubagentParentContext,
        handle: &AgentHandle,
        kinds: Vec<AgentEnvelopeKind>,
    ) -> Result<()> {
        self.append_parent_events(
            parent,
            kinds
                .into_iter()
                .map(|kind| RunEventKind::AgentEnvelope {
                    envelope: AgentEnvelope::new(
                        handle.agent_id.clone(),
                        handle.parent_agent_id.clone(),
                        handle.run_id.clone(),
                        handle.agent_session_id.clone(),
                        kind,
                    ),
                })
                .collect(),
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

    async fn append_spawn_events(
        &self,
        parent: &SubagentParentContext,
        planned: &[PlannedChildSpawn],
    ) -> std::result::Result<(), ToolError> {
        for child in planned {
            let mut events = vec![
                RunEventKind::TaskCreated {
                    task: child.task.clone(),
                    parent_agent_id: parent.parent_agent_id.clone(),
                },
                RunEventKind::AgentEnvelope {
                    envelope: AgentEnvelope::new(
                        child.handle.agent_id.clone(),
                        child.handle.parent_agent_id.clone(),
                        child.handle.run_id.clone(),
                        child.handle.agent_session_id.clone(),
                        AgentEnvelopeKind::SpawnRequested {
                            task: child.task.clone(),
                        },
                    ),
                },
            ];
            if !child.task.requested_write_set.is_empty() {
                events.push(RunEventKind::AgentEnvelope {
                    envelope: AgentEnvelope::new(
                        child.handle.agent_id.clone(),
                        child.handle.parent_agent_id.clone(),
                        child.handle.run_id.clone(),
                        child.handle.agent_session_id.clone(),
                        AgentEnvelopeKind::ClaimRequested {
                            files: child.task.requested_write_set.clone(),
                        },
                    ),
                });
                events.push(RunEventKind::AgentEnvelope {
                    envelope: AgentEnvelope::new(
                        child.handle.agent_id.clone(),
                        child.handle.parent_agent_id.clone(),
                        child.handle.run_id.clone(),
                        child.handle.agent_session_id.clone(),
                        AgentEnvelopeKind::ClaimGranted {
                            files: child.task.requested_write_set.clone(),
                        },
                    ),
                });
            }
            self.append_parent_events(parent, events)
                .await
                .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        }
        Ok(())
    }

    fn resolve_dependency_plan(
        &self,
        parent: &SubagentParentContext,
        planned: &mut [PlannedChildSpawn],
    ) -> std::result::Result<(), ToolError> {
        let mut existing_by_task_id = BTreeMap::new();
        for handle in self
            .session_manager
            .list()
            .into_iter()
            .filter(|handle| handle.parent_agent_id == parent.parent_agent_id)
        {
            if existing_by_task_id
                .insert(handle.task_id.clone(), handle.agent_id.clone())
                .is_some()
            {
                return Err(ToolError::invalid(format!(
                    "duplicate child task id `{}` already exists under parent scope",
                    handle.task_id
                )));
            }
        }

        let mut task_to_index = BTreeMap::new();
        for (index, child) in planned.iter().enumerate() {
            if existing_by_task_id.contains_key(&child.task.task_id) {
                return Err(ToolError::invalid(format!(
                    "child task id `{}` already exists under parent scope",
                    child.task.task_id
                )));
            }
            if task_to_index
                .insert(child.task.task_id.clone(), index)
                .is_some()
            {
                return Err(ToolError::invalid(format!(
                    "duplicate agent task id `{}` in batch spawn",
                    child.task.task_id
                )));
            }
        }

        let handle_ids = planned
            .iter()
            .map(|child| child.handle.agent_id.clone())
            .collect::<Vec<_>>();
        let mut dependency_counts = BTreeMap::new();
        let mut reverse_edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for child in planned.iter_mut() {
            let mut dependency_agent_ids = Vec::with_capacity(child.task.dependency_ids.len());
            let mut intra_batch_dependencies = 0usize;
            for dependency_id in &child.task.dependency_ids {
                if let Some(&dependency_index) = task_to_index.get(dependency_id) {
                    dependency_agent_ids.push(handle_ids[dependency_index].clone());
                    intra_batch_dependencies += 1;
                    reverse_edges
                        .entry(dependency_id.clone())
                        .or_default()
                        .push(child.task.task_id.clone());
                } else if let Some(existing_agent_id) = existing_by_task_id.get(dependency_id) {
                    dependency_agent_ids.push(existing_agent_id.clone());
                } else {
                    return Err(ToolError::invalid(format!(
                        "agent task {} references unknown dependency `{dependency_id}`",
                        child.task.task_id
                    )));
                }
            }
            dependency_counts.insert(child.task.task_id.clone(), intra_batch_dependencies);
            child.dependency_agent_ids = dependency_agent_ids;
        }

        let mut ready = dependency_counts
            .iter()
            .filter_map(|(task_id, count)| (*count == 0).then_some(task_id.clone()))
            .collect::<VecDeque<_>>();
        let mut visited = 0usize;
        while let Some(task_id) = ready.pop_front() {
            visited += 1;
            for dependent in reverse_edges.get(&task_id).into_iter().flatten() {
                if let Some(remaining) = dependency_counts.get_mut(dependent) {
                    *remaining = remaining.saturating_sub(1);
                    if *remaining == 0 {
                        ready.push_back(dependent.clone());
                    }
                }
            }
        }
        if visited != planned.len() {
            return Err(ToolError::invalid(
                "agent batch dependencies contain a cycle",
            ));
        }
        Ok(())
    }

    fn build_child_runtime(&self, plan: &ChildLaunchPlan) -> AgentRuntime {
        AgentRuntimeBuilder::new(plan.profile.backend.clone(), self.store.clone())
            .hook_runner(self.hook_runner.clone())
            .tool_registry(plan.tool_registry.clone())
            .tool_context(plan.profile.tool_context.clone().with_agent_scope_metadata(
                plan.handle.agent_id.clone(),
                Some(plan.task.role.clone()),
                Some(plan.task.task_id.clone()),
                self.write_lease_manager.clone(),
            ))
            .tool_approval_handler(self.tool_approval_handler.clone())
            .tool_approval_policy(self.tool_approval_policy.clone())
            .conversation_compactor(plan.profile.conversation_compactor.clone())
            .compaction_config(plan.profile.compaction_config.clone())
            .loop_detection_config(self.loop_detection_config.clone())
            .instructions(plan.profile.instructions.clone())
            .hooks(self.hooks.clone())
            .skill_catalog(self.skill_catalog.clone())
            .session(RuntimeSession::new(
                plan.handle.run_id.clone(),
                plan.handle.agent_session_id.clone(),
            ))
            .build()
    }

    fn attach_child_worker(&self, plan: ChildLaunchPlan, runtime: AgentRuntime) {
        let handle = plan.handle.clone();
        let worker = ChildAgentWorker {
            parent: plan.parent,
            store: self.store.clone(),
            session_manager: self.session_manager.clone(),
            write_lease_manager: self.write_lease_manager.clone(),
            handle: handle.clone(),
            task: plan.task,
            runtime,
            mailbox_rx: plan.mailbox_rx,
        };
        let join_handle = tokio::spawn(async move { worker.run().await });
        // The record is inserted before any worker is launched, so losing it here
        // would mean internal state corruption rather than a recoverable runtime
        // error.
        self.session_manager
            .attach_join_handle(&handle.agent_id, join_handle)
            .expect("child record inserted before worker launch");
    }

    async fn launch_child_async(&self, plan: ChildLaunchPlan) {
        let failure_parent = plan.parent.clone();
        let failure_handle = plan.handle.clone();
        let failure_task = plan.task.clone();
        // Child startup rebuilds the runtime view, tool registry, and hook stack.
        // Run that work behind a bounded blocking lane so ready batches can warm
        // multiple children at once without unbounded fan-out.
        let prepared = tokio::task::spawn_blocking({
            let executor = self.clone();
            move || {
                let runtime = executor.build_child_runtime(&plan);
                (plan, runtime)
            }
        })
        .await;
        match prepared {
            Ok((plan, runtime)) => self.attach_child_worker(plan, runtime),
            Err(error) => {
                self.fail_launch_child(
                    &failure_parent,
                    failure_handle,
                    failure_task,
                    format!("failed to prepare child runtime: {error}"),
                )
                .await;
            }
        }
    }

    async fn launch_ready_children_bounded(&self, ready: Vec<ChildLaunchPlan>) {
        if ready.is_empty() {
            return;
        }
        let executor = self.clone();
        run_bounded_launches(ready, READY_CHILD_LAUNCH_CONCURRENCY, move |plan| {
            let executor = executor.clone();
            async move { executor.launch_child_async(plan).await }
        })
        .await;
    }

    fn dependency_gate(
        &self,
        dependency_agent_ids: &[AgentId],
    ) -> std::result::Result<DependencyGate, ToolError> {
        let mut pending = BTreeSet::new();
        let mut failures = Vec::new();
        for dependency_agent_id in dependency_agent_ids {
            let snapshot = self
                .session_manager
                .snapshot(dependency_agent_id)
                .map_err(|error| ToolError::invalid_state(error.to_string()))?;
            match snapshot.handle.status {
                AgentStatus::Completed => {}
                AgentStatus::Failed | AgentStatus::Cancelled => failures.push(DependencyFailure {
                    task_id: snapshot.task.task_id,
                    status: snapshot.handle.status,
                    summary: snapshot.result.map(|result| result.summary),
                }),
                _ => {
                    pending.insert(dependency_agent_id.clone());
                }
            }
        }
        if !failures.is_empty() {
            Ok(DependencyGate::Failed(failures))
        } else if pending.is_empty() {
            Ok(DependencyGate::Ready)
        } else {
            Ok(DependencyGate::Blocked(pending))
        }
    }

    async fn run_dependency_scheduler(self, mut blocked: BTreeMap<AgentId, ChildLaunchPlan>) {
        while !blocked.is_empty() {
            let mut ready = Vec::new();
            let mut failed = Vec::new();
            let mut waiting_on = BTreeSet::new();

            for (agent_id, plan) in &blocked {
                let Ok(snapshot) = self.session_manager.snapshot(agent_id) else {
                    ready.push(agent_id.clone());
                    continue;
                };
                if snapshot.handle.status.is_terminal() {
                    ready.push(agent_id.clone());
                    continue;
                }
                match self.dependency_gate(&plan.dependency_agent_ids) {
                    Ok(DependencyGate::Ready) => ready.push(agent_id.clone()),
                    Ok(DependencyGate::Failed(failures)) => {
                        failed.push((agent_id.clone(), failures));
                    }
                    Ok(DependencyGate::Blocked(pending)) => waiting_on.extend(pending),
                    Err(_) => ready.push(agent_id.clone()),
                }
            }

            let mut ready_plans = Vec::new();
            for agent_id in ready {
                if let Some(plan) = blocked.remove(&agent_id) {
                    if self
                        .session_manager
                        .handle(&agent_id)
                        .map(|handle| handle.status.is_terminal())
                        .unwrap_or(true)
                    {
                        continue;
                    }
                    ready_plans.push(plan);
                }
            }
            self.launch_ready_children_bounded(ready_plans).await;

            for (agent_id, failures) in failed {
                if let Some(plan) = blocked.remove(&agent_id) {
                    self.fail_blocked_child(&plan.parent, plan.handle, plan.task, failures)
                        .await;
                }
            }

            if blocked.is_empty() {
                return;
            }
            if waiting_on.is_empty() {
                return;
            }
            let _ = self
                .session_manager
                .wait(AgentWaitRequest {
                    agent_ids: waiting_on.into_iter().collect(),
                    mode: types::AgentWaitMode::Any,
                })
                .await;
        }
    }

    async fn fail_blocked_child(
        &self,
        parent: &SubagentParentContext,
        handle: AgentHandle,
        task: AgentTaskSpec,
        failures: Vec<DependencyFailure>,
    ) {
        let status = if failures
            .iter()
            .any(|failure| failure.status == AgentStatus::Failed)
        {
            AgentStatus::Failed
        } else {
            AgentStatus::Cancelled
        };
        let dependency_summary = failures
            .iter()
            .map(|failure| format!("{}={}", failure.task_id, failure.status))
            .collect::<Vec<_>>()
            .join(", ");
        let summary = format!("dependency gate blocked by {dependency_summary}");
        let details = failures
            .iter()
            .map(|failure| match &failure.summary {
                Some(reason) => format!("{} ({}) {reason}", failure.task_id, failure.status),
                None => format!("{} ({})", failure.task_id, failure.status),
            })
            .collect::<Vec<_>>()
            .join("\n");
        let result = AgentResultEnvelope {
            agent_id: handle.agent_id.clone(),
            task_id: task.task_id.clone(),
            status: status.clone(),
            summary: summary.clone(),
            text: details.clone(),
            artifacts: Vec::new(),
            claimed_files: self.write_lease_manager.claimed_paths(&handle.agent_id),
            structured_payload: None,
        };
        let Ok(handle) = self.session_manager.finish(
            &handle.agent_id,
            status.clone(),
            Some(result.clone()),
            Some(summary.clone()),
        ) else {
            return;
        };
        let released_agent_id = handle.agent_id.clone();
        let _ = self
            .append_agent_envelope(
                parent,
                &handle,
                AgentEnvelopeKind::StatusChanged {
                    status: status.clone(),
                },
            )
            .await;
        match status {
            AgentStatus::Failed => {
                let _ = self
                    .append_agent_envelope(
                        parent,
                        &handle,
                        AgentEnvelopeKind::Failed {
                            error: details.clone(),
                        },
                    )
                    .await;
            }
            AgentStatus::Cancelled => {
                let _ = self
                    .append_agent_envelope(
                        parent,
                        &handle,
                        AgentEnvelopeKind::Cancelled {
                            reason: Some(summary.clone()),
                        },
                    )
                    .await;
            }
            _ => {}
        }
        let _ = self
            .append_agent_envelope(
                parent,
                &handle,
                AgentEnvelopeKind::Result {
                    result: result.clone(),
                },
            )
            .await;
        let _ = self
            .append_parent_event(
                parent,
                RunEventKind::TaskCompleted {
                    task_id: task.task_id.clone(),
                    agent_id: handle.agent_id.clone(),
                    status: status.clone(),
                },
            )
            .await;
        let _ = self
            .append_parent_event(
                parent,
                RunEventKind::SubagentStop {
                    handle,
                    result: Some(result),
                    error: Some(summary),
                },
            )
            .await;
        self.write_lease_manager.release(&released_agent_id);
    }

    async fn fail_launch_child(
        &self,
        parent: &SubagentParentContext,
        handle: AgentHandle,
        task: AgentTaskSpec,
        error: String,
    ) {
        let summary = summarize_output(&error);
        let result = AgentResultEnvelope {
            agent_id: handle.agent_id.clone(),
            task_id: task.task_id.clone(),
            status: AgentStatus::Failed,
            summary: summary.clone(),
            text: error.clone(),
            artifacts: Vec::new(),
            claimed_files: self.write_lease_manager.claimed_paths(&handle.agent_id),
            structured_payload: None,
        };
        let Ok(handle) = self.session_manager.finish(
            &handle.agent_id,
            AgentStatus::Failed,
            Some(result.clone()),
            Some(error.clone()),
        ) else {
            return;
        };
        let released_agent_id = handle.agent_id.clone();
        let _ = self
            .append_agent_envelope(
                parent,
                &handle,
                AgentEnvelopeKind::StatusChanged {
                    status: AgentStatus::Failed,
                },
            )
            .await;
        let _ = self
            .append_agent_envelope(
                parent,
                &handle,
                AgentEnvelopeKind::Failed {
                    error: error.clone(),
                },
            )
            .await;
        let _ = self
            .append_agent_envelope(
                parent,
                &handle,
                AgentEnvelopeKind::Result {
                    result: result.clone(),
                },
            )
            .await;
        let _ = self
            .append_parent_event(
                parent,
                RunEventKind::TaskCompleted {
                    task_id: task.task_id.clone(),
                    agent_id: handle.agent_id.clone(),
                    status: AgentStatus::Failed,
                },
            )
            .await;
        let _ = self
            .append_parent_event(
                parent,
                RunEventKind::SubagentStop {
                    handle,
                    result: Some(result),
                    error: Some(summary),
                },
            )
            .await;
        self.write_lease_manager.release(&released_agent_id);
    }
}

#[async_trait]
impl SubagentExecutor for RuntimeSubagentExecutor {
    async fn spawn(
        &self,
        parent: SubagentParentContext,
        tasks: Vec<AgentTaskSpec>,
    ) -> std::result::Result<Vec<AgentHandle>, ToolError> {
        let mut planned = Vec::new();
        for task in tasks {
            let (tool_registry, resolved_tools) = self
                .resolve_child_tools(&task.allowed_tools)
                .map_err(|error| ToolError::invalid_state(error.to_string()))?;
            let profile = self
                .profile_resolver
                .resolve_profile(&task)
                .map_err(|error| ToolError::invalid_state(error.to_string()))?;
            if !profile.supports_tool_calls && !tool_registry.names().is_empty() {
                return Err(ToolError::invalid(format!(
                    "subagent profile `{}` does not support tool calls, but task `{}` resolved local tools",
                    profile.profile_name, task.task_id
                )));
            }
            let task = AgentTaskSpec {
                allowed_tools: resolved_tools,
                ..task
            };
            let requested_files = self
                .resolve_write_set(&task.requested_write_set)
                .map_err(|error| ToolError::invalid_state(error.to_string()))?;

            planned.push(PlannedChildSpawn {
                handle: AgentHandle {
                    agent_id: AgentId::new(),
                    parent_agent_id: parent.parent_agent_id.clone(),
                    run_id: RunId::new(),
                    agent_session_id: AgentSessionId::new(),
                    task_id: task.task_id.clone(),
                    role: task.role.clone(),
                    status: AgentStatus::Queued,
                },
                task,
                tool_registry,
                requested_files,
                dependency_agent_ids: Vec::new(),
                profile,
            });
        }
        self.resolve_dependency_plan(&parent, &mut planned)?;

        let mut claimed_agents = Vec::new();
        for child in &planned {
            if child.requested_files.is_empty() {
                continue;
            }
            if let Err(conflict) = self
                .write_lease_manager
                .claim(&child.handle.agent_id, &child.requested_files)
            {
                for agent_id in &claimed_agents {
                    self.write_lease_manager.release(agent_id);
                }
                return Err(ToolError::invalid_state(format!(
                    "write lease conflict for {} owned by {}",
                    conflict.requested, conflict.owner
                )));
            }
            claimed_agents.push(child.handle.agent_id.clone());
        }

        if let Err(error) = self.append_spawn_events(&parent, &planned).await {
            for agent_id in &claimed_agents {
                self.write_lease_manager.release(agent_id);
            }
            return Err(error);
        }

        let mut handles = Vec::with_capacity(planned.len());
        let mut ready = Vec::new();
        let mut blocked = BTreeMap::new();
        for child in planned {
            let handle = child.handle.clone();
            let task = child.task.clone();
            let (mailbox, mailbox_rx) = agent_mailbox_channel();
            self.session_manager
                .insert(handle.clone(), task.clone(), mailbox);
            let launch_plan = ChildLaunchPlan {
                parent: parent.clone(),
                handle: handle.clone(),
                task,
                tool_registry: child.tool_registry,
                mailbox_rx,
                dependency_agent_ids: child.dependency_agent_ids,
                profile: child.profile,
            };
            if launch_plan.dependency_agent_ids.is_empty() {
                ready.push(launch_plan);
            } else {
                blocked.insert(handle.agent_id.clone(), launch_plan);
            }
            handles.push(handle);
        }
        self.launch_ready_children_bounded(ready).await;
        if !blocked.is_empty() {
            tokio::spawn(self.clone().run_dependency_scheduler(blocked));
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
            .append_parent_events(vec![
                RunEventKind::SubagentStart {
                    handle: handle.clone(),
                    task: self.task.clone(),
                },
                RunEventKind::AgentEnvelope {
                    envelope: AgentEnvelope::new(
                        self.handle.agent_id.clone(),
                        self.handle.parent_agent_id.clone(),
                        self.handle.run_id.clone(),
                        self.handle.agent_session_id.clone(),
                        AgentEnvelopeKind::StatusChanged {
                            status: AgentStatus::Running,
                        },
                    ),
                },
                RunEventKind::AgentEnvelope {
                    envelope: AgentEnvelope::new(
                        self.handle.agent_id.clone(),
                        self.handle.parent_agent_id.clone(),
                        self.handle.run_id.clone(),
                        self.handle.agent_session_id.clone(),
                        AgentEnvelopeKind::Started {
                            task: self.task.clone(),
                        },
                    ),
                },
            ])
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
        let mut events = vec![RunEventKind::AgentEnvelope {
            envelope: AgentEnvelope::new(
                self.handle.agent_id.clone(),
                self.handle.parent_agent_id.clone(),
                self.handle.run_id.clone(),
                self.handle.agent_session_id.clone(),
                AgentEnvelopeKind::StatusChanged {
                    status: result.status.clone(),
                },
            ),
        }];
        events.extend(result.artifacts.iter().cloned().map(|artifact| {
            RunEventKind::AgentEnvelope {
                envelope: AgentEnvelope::new(
                    self.handle.agent_id.clone(),
                    self.handle.parent_agent_id.clone(),
                    self.handle.run_id.clone(),
                    self.handle.agent_session_id.clone(),
                    AgentEnvelopeKind::Artifact { artifact },
                ),
            }
        }));
        events.push(RunEventKind::AgentEnvelope {
            envelope: AgentEnvelope::new(
                self.handle.agent_id.clone(),
                self.handle.parent_agent_id.clone(),
                self.handle.run_id.clone(),
                self.handle.agent_session_id.clone(),
                AgentEnvelopeKind::Result {
                    result: result.clone(),
                },
            ),
        });
        events.push(RunEventKind::TaskCompleted {
            task_id: self.task.task_id.clone(),
            agent_id: handle.agent_id.clone(),
            status: result.status.clone(),
        });
        events.push(RunEventKind::SubagentStop {
            handle,
            result: Some(result),
            error: None,
        });
        let _ = self.append_parent_events(events).await;
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
            .append_parent_events(vec![
                RunEventKind::AgentEnvelope {
                    envelope: AgentEnvelope::new(
                        self.handle.agent_id.clone(),
                        self.handle.parent_agent_id.clone(),
                        self.handle.run_id.clone(),
                        self.handle.agent_session_id.clone(),
                        AgentEnvelopeKind::StatusChanged {
                            status: AgentStatus::Cancelled,
                        },
                    ),
                },
                RunEventKind::AgentEnvelope {
                    envelope: AgentEnvelope::new(
                        self.handle.agent_id.clone(),
                        self.handle.parent_agent_id.clone(),
                        self.handle.run_id.clone(),
                        self.handle.agent_session_id.clone(),
                        AgentEnvelopeKind::Cancelled {
                            reason: reason.clone(),
                        },
                    ),
                },
                RunEventKind::TaskCompleted {
                    task_id: self.task.task_id.clone(),
                    agent_id: handle.agent_id.clone(),
                    status: AgentStatus::Cancelled,
                },
                RunEventKind::SubagentStop {
                    handle,
                    result: Some(result),
                    error: reason.clone(),
                },
            ])
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
            .append_parent_events(vec![
                RunEventKind::AgentEnvelope {
                    envelope: AgentEnvelope::new(
                        self.handle.agent_id.clone(),
                        self.handle.parent_agent_id.clone(),
                        self.handle.run_id.clone(),
                        self.handle.agent_session_id.clone(),
                        AgentEnvelopeKind::StatusChanged {
                            status: AgentStatus::Failed,
                        },
                    ),
                },
                RunEventKind::AgentEnvelope {
                    envelope: AgentEnvelope::new(
                        self.handle.agent_id.clone(),
                        self.handle.parent_agent_id.clone(),
                        self.handle.run_id.clone(),
                        self.handle.agent_session_id.clone(),
                        AgentEnvelopeKind::Failed {
                            error: error.clone(),
                        },
                    ),
                },
                RunEventKind::TaskCompleted {
                    task_id: self.task.task_id.clone(),
                    agent_id: handle.agent_id.clone(),
                    status: AgentStatus::Failed,
                },
                RunEventKind::SubagentStop {
                    handle,
                    result: Some(result),
                    error: Some(error),
                },
            ])
            .await;
        self.write_lease_manager.release(&self.handle.agent_id);
        let _ = self.runtime.end_session(Some("failed".to_string())).await;
    }

    async fn append_parent_events(&self, events: Vec<RunEventKind>) -> Result<()> {
        let Some(run_id) = self.parent.run_id.clone() else {
            return Ok(());
        };
        let Some(agent_session_id) = self.parent.agent_session_id.clone() else {
            return Ok(());
        };
        self.store
            .append_batch(
                events
                    .into_iter()
                    .map(|event| {
                        RunEventEnvelope::new(
                            run_id.clone(),
                            agent_session_id.clone(),
                            self.parent.turn_id.clone(),
                            None,
                            event,
                        )
                    })
                    .collect(),
            )
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
    use super::{
        RuntimeSubagentExecutor, SubagentProfileResolver, SubagentRuntimeProfile,
        normalize_child_result, run_bounded_launches,
    };
    use crate::Result;
    use crate::{
        AlwaysAllowToolApprovalHandler, CompactionConfig, HookRunner, LoopDetectionConfig,
        ModelBackend, NoopConversationCompactor, NoopToolApprovalPolicy,
    };
    use async_trait::async_trait;
    use futures::{StreamExt, stream, stream::BoxStream};
    use skills::SkillCatalog;
    use std::sync::atomic::{AtomicUsize, Ordering};
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

    #[derive(Clone)]
    struct StaticProfileResolver {
        backend: Arc<dyn ModelBackend>,
        tool_context: ToolExecutionContext,
        supports_tool_calls: bool,
    }

    impl StaticProfileResolver {
        fn new(
            backend: Arc<dyn ModelBackend>,
            tool_context: ToolExecutionContext,
            supports_tool_calls: bool,
        ) -> Self {
            Self {
                backend,
                tool_context,
                supports_tool_calls,
            }
        }
    }

    impl SubagentProfileResolver for StaticProfileResolver {
        fn resolve_profile(&self, task: &AgentTaskSpec) -> Result<SubagentRuntimeProfile> {
            Ok(SubagentRuntimeProfile {
                profile_name: format!("roles.{}", task.role),
                backend: self.backend.clone(),
                tool_context: self.tool_context.clone(),
                conversation_compactor: Arc::new(NoopConversationCompactor),
                compaction_config: CompactionConfig::default(),
                instructions: vec![format!("profile instruction for {}", task.role)],
                supports_tool_calls: self.supports_tool_calls,
            })
        }
    }

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
                    usage: None,
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
                    usage: None,
                    reasoning: Vec::new(),
                }),
            ])
            .boxed())
        }
    }

    #[derive(Clone, Default)]
    struct ConditionalBackend {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    #[async_trait]
    impl ModelBackend for ConditionalBackend {
        async fn stream_turn(
            &self,
            request: ModelRequest,
        ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
            self.requests.lock().unwrap().push(request.clone());
            let user_prompt = request
                .messages
                .iter()
                .find(|message| message.role == MessageRole::User)
                .map(|message| message.text_content())
                .unwrap_or_default();
            if user_prompt.contains("fail dependency") {
                return Err(crate::RuntimeError::invalid_state(
                    "dependency execution failed",
                ));
            }
            Ok(stream::iter(vec![
                Ok(ModelEvent::TextDelta {
                    delta: format!("child ok {user_prompt}"),
                }),
                Ok(ModelEvent::ResponseComplete {
                    stop_reason: Some("stop".to_string()),
                    message_id: None,
                    continuation: None,
                    usage: None,
                    reasoning: Vec::new(),
                }),
            ])
            .boxed())
        }
    }

    #[derive(Clone, Default)]
    struct FailingBackend {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    #[async_trait]
    impl ModelBackend for FailingBackend {
        async fn stream_turn(
            &self,
            request: ModelRequest,
        ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
            self.requests.lock().unwrap().push(request.clone());
            let prompt = request
                .messages
                .iter()
                .find(|message| message.role == MessageRole::User)
                .map(|message| message.text_content())
                .unwrap_or_default();
            if prompt.contains("fail") {
                return Ok(stream::iter(vec![Ok(ModelEvent::Error {
                    message: "upstream failed".to_string(),
                })])
                .boxed());
            }
            Ok(stream::iter(vec![
                Ok(ModelEvent::TextDelta {
                    delta: format!("child ok {prompt}"),
                }),
                Ok(ModelEvent::ResponseComplete {
                    stop_reason: Some("stop".to_string()),
                    message_id: None,
                    continuation: None,
                    usage: None,
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
        make_executor_with_tool_calls(backend, store, true)
    }

    fn make_executor_with_tool_calls(
        backend: Arc<dyn ModelBackend>,
        store: Arc<dyn RunStore>,
        supports_tool_calls: bool,
    ) -> RuntimeSubagentExecutor {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = ToolRegistry::new();
        registry.register(ReadTool::new());
        let tool_context = ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            workspace_only: true,
            ..Default::default()
        };
        RuntimeSubagentExecutor::new(
            Arc::new(HookRunner::default()),
            store,
            registry,
            tool_context.clone(),
            Arc::new(AlwaysAllowToolApprovalHandler),
            Arc::new(NoopToolApprovalPolicy),
            LoopDetectionConfig::default(),
            Vec::new(),
            SkillCatalog::default(),
            Arc::new(StaticProfileResolver::new(
                backend,
                tool_context,
                supports_tool_calls,
            )),
        )
    }

    #[tokio::test]
    async fn runtime_subagent_executor_spawns_batch_and_waits_all() {
        let backend = Arc::new(ImmediateBackend::default());
        let store = Arc::new(InMemoryRunStore::new());
        let executor = make_executor(backend, store);
        let parent = SubagentParentContext {
            run_id: Some("run_parent".into()),
            agent_session_id: Some("session_parent".into()),
            turn_id: Some("turn_parent".into()),
            parent_agent_id: Some("agent_parent".into()),
        };

        let handles = executor
            .spawn(
                parent.clone(),
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
                parent,
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
    async fn runtime_subagent_executor_uses_role_specific_profile_instructions() {
        let backend = Arc::new(ImmediateBackend::default());
        let store = Arc::new(InMemoryRunStore::new());
        let executor = make_executor(backend.clone(), store);

        let handles = executor
            .spawn(
                SubagentParentContext::default(),
                vec![AgentTaskSpec {
                    task_id: "review".to_string(),
                    role: "reviewer".to_string(),
                    prompt: "review".to_string(),
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

        let requests = backend.requests.lock().unwrap();
        assert!(requests.iter().any(|request| {
            request
                .instructions
                .iter()
                .any(|instruction| instruction.contains("profile instruction for reviewer"))
        }));
    }

    #[tokio::test]
    async fn runtime_subagent_executor_fails_fast_when_profile_disables_tool_calls() {
        let backend = Arc::new(ImmediateBackend::default());
        let store = Arc::new(InMemoryRunStore::new());
        let executor = make_executor_with_tool_calls(backend, store, false);

        let error = executor
            .spawn(
                SubagentParentContext::default(),
                vec![AgentTaskSpec {
                    task_id: "review".to_string(),
                    role: "reviewer".to_string(),
                    prompt: "review".to_string(),
                    steer: None,
                    allowed_tools: vec![ToolName::from("read")],
                    requested_write_set: Vec::new(),
                    dependency_ids: Vec::new(),
                    timeout_seconds: None,
                }],
            )
            .await
            .expect_err("tool-capability mismatch must fail fast");

        assert!(error.to_string().contains("does not support tool calls"));
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
            agent_session_id: Some("session_parent".into()),
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
    async fn runtime_subagent_executor_blocks_dependent_child_until_dependency_finishes() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let backend = Arc::new(BlockingBackend {
            requests: Arc::new(Mutex::new(Vec::new())),
            started: started.clone(),
            release: release.clone(),
            first_user_request_pending: Arc::new(Mutex::new(true)),
        });
        let store = Arc::new(InMemoryRunStore::new());
        let executor = make_executor(backend.clone(), store);

        let handles = executor
            .spawn(
                SubagentParentContext::default(),
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
                        dependency_ids: vec!["inspect".to_string()],
                        timeout_seconds: None,
                    },
                ],
            )
            .await
            .unwrap();

        started.notified().await;
        tokio::task::yield_now().await;

        assert_eq!(backend.requests.lock().unwrap().len(), 1);
        let review_handle = handles
            .iter()
            .find(|handle| handle.task_id == "review")
            .unwrap();
        assert_eq!(
            executor
                .session_manager
                .handle(&review_handle.agent_id)
                .unwrap()
                .status,
            AgentStatus::Queued
        );

        release.notify_waiters();
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

        assert_eq!(backend.requests.lock().unwrap().len(), 2);
        assert_eq!(wait.completed.len(), 2);
        assert!(wait.pending.is_empty());
        assert!(
            wait.results
                .iter()
                .all(|result| result.status == AgentStatus::Completed)
        );
    }

    #[tokio::test]
    async fn runtime_subagent_executor_propagates_dependency_failures_without_launching_child() {
        let backend = Arc::new(ConditionalBackend::default());
        let store = Arc::new(InMemoryRunStore::new());
        let executor = make_executor(backend.clone(), store);

        let handles = executor
            .spawn(
                SubagentParentContext::default(),
                vec![
                    AgentTaskSpec {
                        task_id: "inspect".to_string(),
                        role: "explorer".to_string(),
                        prompt: "fail dependency".to_string(),
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
                        dependency_ids: vec!["inspect".to_string()],
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

        assert_eq!(backend.requests.lock().unwrap().len(), 1);
        let review_result = wait
            .results
            .iter()
            .find(|result| result.task_id == "review")
            .unwrap();
        assert_eq!(review_result.status, AgentStatus::Failed);
        assert!(review_result.summary.contains("dependency gate blocked"));
        assert!(review_result.text.contains("inspect (failed)"));
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
            agent_session_id: Some("session_parent".into()),
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
    async fn runtime_subagent_executor_batch_spawn_is_atomic_on_conflict() {
        let backend = Arc::new(ImmediateBackend::default());
        let store = Arc::new(InMemoryRunStore::new());
        let executor = make_executor(backend, store);

        let error = executor
            .spawn(
                SubagentParentContext::default(),
                vec![
                    AgentTaskSpec {
                        task_id: "one".to_string(),
                        role: "writer".to_string(),
                        prompt: "write".to_string(),
                        steer: None,
                        allowed_tools: vec![ToolName::from("read")],
                        requested_write_set: vec!["src".to_string()],
                        dependency_ids: Vec::new(),
                        timeout_seconds: None,
                    },
                    AgentTaskSpec {
                        task_id: "two".to_string(),
                        role: "writer".to_string(),
                        prompt: "write".to_string(),
                        steer: None,
                        allowed_tools: vec![ToolName::from("read")],
                        requested_write_set: vec!["src/lib.rs".to_string()],
                        dependency_ids: Vec::new(),
                        timeout_seconds: None,
                    },
                ],
            )
            .await
            .expect_err("conflicting batch must fail");
        assert!(error.to_string().contains("write lease conflict"));

        let root_handles = executor
            .list(SubagentParentContext::default())
            .await
            .unwrap();
        assert!(
            root_handles.is_empty(),
            "failed batch must not leave running children"
        );

        let handles = executor
            .spawn(
                SubagentParentContext::default(),
                vec![AgentTaskSpec {
                    task_id: "retry".to_string(),
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
            .expect("claims must be released after failed batch");
        assert_eq!(handles.len(), 1);
    }

    #[tokio::test]
    async fn runtime_subagent_executor_defers_downstream_until_dependencies_complete() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let backend = Arc::new(BlockingBackend {
            requests: Arc::new(Mutex::new(Vec::new())),
            started: started.clone(),
            release: release.clone(),
            first_user_request_pending: Arc::new(Mutex::new(true)),
        });
        let store = Arc::new(InMemoryRunStore::new());
        let executor = make_executor(backend.clone(), store);
        let parent = SubagentParentContext {
            run_id: Some("run_parent".into()),
            agent_session_id: Some("session_parent".into()),
            turn_id: Some("turn_parent".into()),
            parent_agent_id: Some("agent_parent".into()),
        };

        let handles = executor
            .spawn(
                parent.clone(),
                vec![
                    AgentTaskSpec {
                        task_id: "inspect".to_string(),
                        role: "explorer".to_string(),
                        prompt: "inspect workspace".to_string(),
                        steer: None,
                        allowed_tools: vec![ToolName::from("read")],
                        requested_write_set: Vec::new(),
                        dependency_ids: Vec::new(),
                        timeout_seconds: None,
                    },
                    AgentTaskSpec {
                        task_id: "review".to_string(),
                        role: "reviewer".to_string(),
                        prompt: "review findings".to_string(),
                        steer: None,
                        allowed_tools: vec![ToolName::from("read")],
                        requested_write_set: Vec::new(),
                        dependency_ids: vec!["inspect".to_string()],
                        timeout_seconds: None,
                    },
                ],
            )
            .await
            .unwrap();

        started.notified().await;
        assert_eq!(backend.requests.lock().unwrap().len(), 1);
        let listed = executor.list(parent.clone()).await.unwrap();
        assert_eq!(
            listed
                .iter()
                .find(|handle| handle.task_id == "review")
                .map(|handle| handle.status.clone()),
            Some(AgentStatus::Queued)
        );

        release.notify_waiters();
        let wait = executor
            .wait(
                parent,
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

        assert_eq!(backend.requests.lock().unwrap().len(), 2);
        assert_eq!(wait.results.len(), 2);
        assert!(
            wait.results
                .iter()
                .all(|result| result.status == AgentStatus::Completed)
        );
        let prompts = backend
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| {
                request
                    .messages
                    .iter()
                    .find(|message| message.role == MessageRole::User)
                    .map(|message| message.text_content())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        assert_eq!(prompts, vec!["inspect workspace", "review findings"]);
    }

    #[tokio::test]
    async fn runtime_subagent_executor_fails_downstream_when_dependency_fails() {
        let backend = Arc::new(FailingBackend::default());
        let store = Arc::new(InMemoryRunStore::new());
        let executor = make_executor(backend.clone(), store);
        let parent = SubagentParentContext {
            run_id: Some("run_parent".into()),
            agent_session_id: Some("session_parent".into()),
            turn_id: Some("turn_parent".into()),
            parent_agent_id: Some("agent_parent".into()),
        };

        let handles = executor
            .spawn(
                parent.clone(),
                vec![
                    AgentTaskSpec {
                        task_id: "inspect".to_string(),
                        role: "explorer".to_string(),
                        prompt: "fail upstream".to_string(),
                        steer: None,
                        allowed_tools: vec![ToolName::from("read")],
                        requested_write_set: Vec::new(),
                        dependency_ids: Vec::new(),
                        timeout_seconds: None,
                    },
                    AgentTaskSpec {
                        task_id: "review".to_string(),
                        role: "reviewer".to_string(),
                        prompt: "review findings".to_string(),
                        steer: None,
                        allowed_tools: vec![ToolName::from("read")],
                        requested_write_set: Vec::new(),
                        dependency_ids: vec!["inspect".to_string()],
                        timeout_seconds: None,
                    },
                ],
            )
            .await
            .unwrap();

        let wait = executor
            .wait(
                parent,
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

        assert_eq!(backend.requests.lock().unwrap().len(), 1);
        let upstream = wait
            .results
            .iter()
            .find(|result| result.task_id == "inspect")
            .unwrap();
        assert_eq!(upstream.status, AgentStatus::Failed);
        let downstream = wait
            .results
            .iter()
            .find(|result| result.task_id == "review")
            .unwrap();
        assert_eq!(downstream.status, AgentStatus::Failed);
        assert!(
            downstream
                .summary
                .contains("dependency gate blocked by inspect=failed"),
            "{}",
            downstream.summary
        );
    }

    #[tokio::test]
    async fn runtime_subagent_executor_blocks_dependent_children_until_ready() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let backend = Arc::new(BlockingBackend {
            requests: Arc::new(Mutex::new(Vec::new())),
            started: started.clone(),
            release: release.clone(),
            first_user_request_pending: Arc::new(Mutex::new(true)),
        });
        let store = Arc::new(InMemoryRunStore::new());
        let executor = make_executor(backend.clone(), store);

        let handles = executor
            .spawn(
                SubagentParentContext::default(),
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
                        dependency_ids: vec!["inspect".to_string()],
                        timeout_seconds: None,
                    },
                ],
            )
            .await
            .unwrap();

        started.notified().await;
        let listing = executor
            .list(SubagentParentContext::default())
            .await
            .unwrap();
        assert_eq!(listing.len(), 2);
        assert_eq!(
            listing
                .iter()
                .find(|handle| handle.task_id == "inspect")
                .unwrap()
                .status,
            AgentStatus::Running
        );
        assert_eq!(
            listing
                .iter()
                .find(|handle| handle.task_id == "review")
                .unwrap()
                .status,
            AgentStatus::Queued
        );
        assert_eq!(
            backend.requests.lock().unwrap().len(),
            1,
            "downstream child must not start before its dependency finishes"
        );

        release.notify_waiters();
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
        assert_eq!(
            backend.requests.lock().unwrap().len(),
            2,
            "downstream child should start after dependency completion"
        );
        assert!(
            wait.results
                .iter()
                .all(|result| result.status == AgentStatus::Completed)
        );
    }

    #[tokio::test]
    async fn runtime_subagent_executor_fails_dependents_after_upstream_failure() {
        let backend = Arc::new(ConditionalBackend::default());
        let store = Arc::new(InMemoryRunStore::new());
        let executor = make_executor(backend.clone(), store);

        let handles = executor
            .spawn(
                SubagentParentContext::default(),
                vec![
                    AgentTaskSpec {
                        task_id: "inspect".to_string(),
                        role: "explorer".to_string(),
                        prompt: "fail dependency".to_string(),
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
                        dependency_ids: vec!["inspect".to_string()],
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

        assert_eq!(backend.requests.lock().unwrap().len(), 1);
        let by_task = wait
            .results
            .iter()
            .map(|result| (result.task_id.as_str(), result))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(by_task["inspect"].status, AgentStatus::Failed);
        assert_eq!(by_task["review"].status, AgentStatus::Failed);
        assert!(
            by_task["review"]
                .summary
                .contains("dependency gate blocked by inspect=failed")
        );
    }

    #[tokio::test]
    async fn runtime_subagent_executor_rejects_dependency_cycles() {
        let backend = Arc::new(ImmediateBackend::default());
        let store = Arc::new(InMemoryRunStore::new());
        let executor = make_executor(backend, store);

        let error = executor
            .spawn(
                SubagentParentContext::default(),
                vec![
                    AgentTaskSpec {
                        task_id: "a".to_string(),
                        role: "explorer".to_string(),
                        prompt: "a".to_string(),
                        steer: None,
                        allowed_tools: vec![ToolName::from("read")],
                        requested_write_set: Vec::new(),
                        dependency_ids: vec!["b".to_string()],
                        timeout_seconds: None,
                    },
                    AgentTaskSpec {
                        task_id: "b".to_string(),
                        role: "reviewer".to_string(),
                        prompt: "b".to_string(),
                        steer: None,
                        allowed_tools: vec![ToolName::from("read")],
                        requested_write_set: Vec::new(),
                        dependency_ids: vec!["a".to_string()],
                        timeout_seconds: None,
                    },
                ],
            )
            .await
            .expect_err("cyclic dependencies must be rejected");

        assert!(error.to_string().contains("dependenc"));
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
            agent_session_id: Some("session_parent".into()),
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

    #[tokio::test]
    async fn run_bounded_launches_caps_parallelism() {
        let started = Arc::new(AtomicUsize::new(0));
        let current = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let reached_limit = Arc::new(Notify::new());

        let worker = tokio::spawn({
            let started = started.clone();
            let current = current.clone();
            let max_seen = max_seen.clone();
            let release = release.clone();
            let reached_limit = reached_limit.clone();
            async move {
                run_bounded_launches(vec![1, 2, 3, 4], 2, move |_| {
                    let started = started.clone();
                    let current = current.clone();
                    let max_seen = max_seen.clone();
                    let release = release.clone();
                    let reached_limit = reached_limit.clone();
                    async move {
                        let in_flight = current.fetch_add(1, Ordering::SeqCst) + 1;
                        let _ = max_seen.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |seen| {
                            (seen < in_flight).then_some(in_flight)
                        });
                        if started.fetch_add(1, Ordering::SeqCst) + 1 == 2 {
                            reached_limit.notify_waiters();
                        }
                        let permit = release.acquire().await.unwrap();
                        drop(permit);
                        current.fetch_sub(1, Ordering::SeqCst);
                    }
                })
                .await;
            }
        });

        reached_limit.notified().await;
        tokio::task::yield_now().await;
        assert_eq!(started.load(Ordering::SeqCst), 2);
        assert_eq!(max_seen.load(Ordering::SeqCst), 2);

        release.add_permits(4);
        worker.await.unwrap();
        assert_eq!(max_seen.load(Ordering::SeqCst), 2);
    }
}
