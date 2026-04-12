use crate::backend::SessionEventStream;
use crate::ui::SessionEvent;
use agent::tools::{
    ChildWorktreeRequest, ExecRequest, ExecutionOrigin, PRIMARY_WORKTREE_ID, ProcessExecutor,
    ProcessStdio, Result as ToolResult, SandboxPolicy, ToolError, ToolExecutionContext,
    WorktreeEnterRequest, WorktreeManager, WorktreeRuntimeContext,
};
use agent::types::{
    AgentSessionId, SessionEventEnvelope, SessionEventKind, SessionId, WorktreeId, WorktreeScope,
    WorktreeStatus, WorktreeSummaryRecord, new_opaque_id,
};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, RwLock};
use store::{SessionStore, SessionStoreError};

#[derive(Clone, Debug)]
struct SessionWorktreeState {
    current_worktree_id: WorktreeId,
    entries: BTreeMap<WorktreeId, WorktreeSummaryRecord>,
}

#[derive(Clone)]
pub struct SessionWorktreeManager {
    store: Arc<dyn SessionStore>,
    events: SessionEventStream,
    process_executor: Arc<dyn ProcessExecutor>,
    primary_root: PathBuf,
    session_tool_context: Arc<RwLock<ToolExecutionContext>>,
    states: Arc<Mutex<BTreeMap<SessionId, SessionWorktreeState>>>,
}

impl SessionWorktreeManager {
    #[must_use]
    pub fn new(
        store: Arc<dyn SessionStore>,
        events: SessionEventStream,
        process_executor: Arc<dyn ProcessExecutor>,
        primary_root: PathBuf,
        session_tool_context: Arc<RwLock<ToolExecutionContext>>,
    ) -> Self {
        Self {
            store,
            events,
            process_executor,
            primary_root,
            session_tool_context,
            states: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub async fn sync_attached_session(
        &self,
        session_id: SessionId,
        agent_session_id: AgentSessionId,
    ) -> ToolResult<()> {
        let runtime = WorktreeRuntimeContext {
            session_id: Some(session_id),
            agent_session_id: Some(agent_session_id),
            ..Default::default()
        };
        let state = self.load_or_restore_state(&runtime).await?;
        let current = state
            .entries
            .get(&state.current_worktree_id)
            .cloned()
            .unwrap_or_else(|| self.primary_summary(&runtime, WorktreeStatus::Active, None));
        self.apply_active_context(&current);
        Ok(())
    }

    fn require_attached_runtime(
        runtime: &WorktreeRuntimeContext,
    ) -> ToolResult<(SessionId, AgentSessionId)> {
        let session_id = runtime.session_id.clone().ok_or_else(|| {
            ToolError::invalid_state("worktree tools require an attached runtime session")
        })?;
        let agent_session_id = runtime.agent_session_id.clone().ok_or_else(|| {
            ToolError::invalid_state("worktree tools require an attached runtime agent session")
        })?;
        Ok((session_id, agent_session_id))
    }

    fn primary_summary(
        &self,
        runtime: &WorktreeRuntimeContext,
        status: WorktreeStatus,
        updated_at_unix_s: Option<u64>,
    ) -> WorktreeSummaryRecord {
        WorktreeSummaryRecord {
            worktree_id: WorktreeId::from(PRIMARY_WORKTREE_ID),
            session_id: runtime
                .session_id
                .clone()
                .expect("primary worktree summary requires session id"),
            agent_session_id: runtime
                .agent_session_id
                .clone()
                .expect("primary worktree summary requires agent session id"),
            scope: WorktreeScope::Session,
            status,
            root: self.primary_root.clone(),
            parent_agent_id: runtime.parent_agent_id.clone(),
            task_id: runtime.task_id.clone(),
            child_agent_id: None,
            label: Some("primary".to_string()),
            created_at_unix_s: unix_timestamp_s(),
            updated_at_unix_s,
        }
    }

    async fn load_or_restore_state(
        &self,
        runtime: &WorktreeRuntimeContext,
    ) -> ToolResult<SessionWorktreeState> {
        let (session_id, _) = Self::require_attached_runtime(runtime)?;
        if let Some(state) = self
            .states
            .lock()
            .expect("worktree state lock")
            .get(&session_id)
            .cloned()
        {
            return Ok(state);
        }

        let mut state = SessionWorktreeState {
            current_worktree_id: WorktreeId::from(PRIMARY_WORKTREE_ID),
            entries: BTreeMap::from([(
                WorktreeId::from(PRIMARY_WORKTREE_ID),
                self.primary_summary(runtime, WorktreeStatus::Active, None),
            )]),
        };
        let events = self
            .store
            .events(&session_id)
            .await
            .or_else(|error| match error {
                SessionStoreError::SessionNotFound(_) => Ok(Vec::new()),
                other => Err(other),
            })
            .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        for envelope in events {
            match envelope.event {
                SessionEventKind::WorktreeEntered { summary }
                | SessionEventKind::WorktreeUpdated { summary } => {
                    if summary.status.is_active() && summary.scope == WorktreeScope::Session {
                        state.current_worktree_id = summary.worktree_id.clone();
                    }
                    state.entries.insert(summary.worktree_id.clone(), summary);
                }
                _ => {}
            }
        }
        self.states
            .lock()
            .expect("worktree state lock")
            .insert(session_id, state.clone());
        Ok(state)
    }

    async fn append_session_event(
        &self,
        runtime: &WorktreeRuntimeContext,
        event: SessionEventKind,
    ) -> ToolResult<()> {
        let (session_id, agent_session_id) = Self::require_attached_runtime(runtime)?;
        self.store
            .append(SessionEventEnvelope::new(
                session_id,
                agent_session_id,
                runtime.turn_id.clone(),
                None,
                event,
            ))
            .await
            .map_err(|error| ToolError::invalid_state(error.to_string()))
    }

    async fn publish_entered(
        &self,
        runtime: &WorktreeRuntimeContext,
        summary: WorktreeSummaryRecord,
    ) -> ToolResult<()> {
        self.append_session_event(
            runtime,
            SessionEventKind::WorktreeEntered {
                summary: summary.clone(),
            },
        )
        .await?;
        self.events
            .publish(SessionEvent::WorktreeEntered { summary });
        Ok(())
    }

    async fn publish_updated(
        &self,
        runtime: &WorktreeRuntimeContext,
        summary: WorktreeSummaryRecord,
    ) -> ToolResult<()> {
        self.append_session_event(
            runtime,
            SessionEventKind::WorktreeUpdated {
                summary: summary.clone(),
            },
        )
        .await?;
        self.events
            .publish(SessionEvent::WorktreeUpdated { summary });
        Ok(())
    }

    async fn run_git(&self, cwd: &Path, args: &[&str]) -> ToolResult<String> {
        let mut command = self
            .process_executor
            .prepare(ExecRequest {
                program: "git".to_string(),
                args: args.iter().map(|value| (*value).to_string()).collect(),
                cwd: Some(cwd.to_path_buf()),
                env: BTreeMap::new(),
                stdin: ProcessStdio::Null,
                stdout: ProcessStdio::Piped,
                stderr: ProcessStdio::Piped,
                kill_on_drop: true,
                origin: ExecutionOrigin::HostUtility {
                    name: "worktree_manager".to_string(),
                },
                runtime_scope: Default::default(),
                // Worktree lifecycle is host-owned control-plane state. It may
                // need to create sibling checkout directories outside the
                // currently active workspace root, so it cannot reuse the
                // turn-local workspace-write policy verbatim.
                sandbox_policy: SandboxPolicy::permissive(),
            })
            .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        let output = command
            .spawn()
            .map_err(|error| ToolError::invalid_state(format!("failed to start git: {error}")))?
            .wait_with_output()
            .await
            .map_err(|error| {
                ToolError::invalid_state(format!("failed to wait for git: {error}"))
            })?;
        if !output.status.success() {
            return Err(ToolError::invalid_state(format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn apply_active_context(&self, summary: &WorktreeSummaryRecord) {
        let mut tool_context = self.session_tool_context.write().unwrap();
        let previous_policy = tool_context.sandbox_policy();
        tool_context.workspace_root = summary.root.clone();
        tool_context.worktree_root = Some(summary.root.clone());
        tool_context.active_worktree_id = Some(summary.worktree_id.clone());
        tool_context.effective_sandbox_policy = Some(
            if matches!(
                previous_policy.mode,
                agent::tools::SandboxMode::DangerFullAccess
            ) {
                SandboxPolicy::permissive()
                    .with_fail_if_unavailable(previous_policy.fail_if_unavailable)
            } else {
                tool_context
                    .sandbox_scope()
                    .recommended_policy()
                    .with_fail_if_unavailable(previous_policy.fail_if_unavailable)
            },
        );
    }

    fn worktree_parent_dir(&self) -> PathBuf {
        let workspace_name = self
            .primary_root
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("workspace");
        self.primary_root
            .parent()
            .unwrap_or(self.primary_root.as_path())
            .join(".nanoclaw-worktrees")
            .join(workspace_name)
    }

    fn make_worktree_path(&self, label: Option<&str>) -> PathBuf {
        let base = sanitize_worktree_label(label.unwrap_or("session"));
        let slug = format!("{base}-{}", &new_opaque_id()[..8]);
        self.worktree_parent_dir().join(slug)
    }

    async fn create_git_worktree(
        &self,
        label: Option<&str>,
        default_label: &str,
    ) -> ToolResult<PathBuf> {
        let root = self.make_worktree_path(label.or(Some(default_label)));
        tokio::fs::create_dir_all(self.worktree_parent_dir())
            .await
            .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        let root_string = root.to_string_lossy().to_string();
        self.run_git(
            &self.primary_root,
            &["worktree", "add", "--detach", &root_string, "HEAD"],
        )
        .await?;
        Ok(root)
    }

    fn normalized_label(label: Option<String>) -> Option<String> {
        label.and_then(|value| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
    }

    async fn remove_active_non_primary_if_needed(
        &self,
        runtime: &WorktreeRuntimeContext,
        state: &mut SessionWorktreeState,
    ) -> ToolResult<()> {
        if state.current_worktree_id.as_str() == PRIMARY_WORKTREE_ID {
            return Ok(());
        }
        let current_id = state.current_worktree_id.clone();
        let summary = state
            .entries
            .get(&current_id)
            .cloned()
            .ok_or_else(|| ToolError::invalid_state("missing current worktree state"))?;
        self.remove_session_worktree(runtime, state, &summary).await
    }

    async fn remove_session_worktree(
        &self,
        runtime: &WorktreeRuntimeContext,
        state: &mut SessionWorktreeState,
        summary: &WorktreeSummaryRecord,
    ) -> ToolResult<()> {
        if summary.worktree_id.as_str() == PRIMARY_WORKTREE_ID {
            return Err(ToolError::invalid("cannot remove the primary worktree"));
        }
        let root_string = summary.root.to_string_lossy().to_string();
        self.run_git(
            &self.primary_root,
            &["worktree", "remove", "--force", &root_string],
        )
        .await?;
        let mut removed = summary.clone();
        removed.status = WorktreeStatus::Removed;
        removed.updated_at_unix_s = Some(unix_timestamp_s());
        state
            .entries
            .insert(removed.worktree_id.clone(), removed.clone());
        let primary =
            self.primary_summary(runtime, WorktreeStatus::Active, Some(unix_timestamp_s()));
        state.current_worktree_id = primary.worktree_id.clone();
        state
            .entries
            .insert(primary.worktree_id.clone(), primary.clone());
        self.apply_active_context(&primary);
        self.publish_updated(runtime, removed).await?;
        self.publish_updated(runtime, primary).await?;
        Ok(())
    }
}

#[async_trait]
impl WorktreeManager for SessionWorktreeManager {
    async fn enter_worktree(
        &self,
        runtime: WorktreeRuntimeContext,
        request: WorktreeEnterRequest,
    ) -> ToolResult<WorktreeSummaryRecord> {
        let (session_id, agent_session_id) = Self::require_attached_runtime(&runtime)?;
        let mut state = self.load_or_restore_state(&runtime).await?;
        self.remove_active_non_primary_if_needed(&runtime, &mut state)
            .await?;
        let root = self
            .create_git_worktree(request.label.as_deref(), "session")
            .await?;
        let mut primary = state
            .entries
            .get(&WorktreeId::from(PRIMARY_WORKTREE_ID))
            .cloned()
            .unwrap_or_else(|| self.primary_summary(&runtime, WorktreeStatus::Inactive, None));
        primary.status = WorktreeStatus::Inactive;
        primary.updated_at_unix_s = Some(unix_timestamp_s());
        state
            .entries
            .insert(primary.worktree_id.clone(), primary.clone());
        self.publish_updated(&runtime, primary).await?;

        let summary = WorktreeSummaryRecord {
            worktree_id: WorktreeId::from(format!("worktree_{}", new_opaque_id())),
            session_id: session_id.clone(),
            agent_session_id,
            scope: WorktreeScope::Session,
            status: WorktreeStatus::Active,
            root,
            parent_agent_id: runtime.parent_agent_id.clone(),
            task_id: runtime.task_id.clone(),
            child_agent_id: None,
            label: Self::normalized_label(request.label),
            created_at_unix_s: unix_timestamp_s(),
            updated_at_unix_s: None,
        };
        state.current_worktree_id = summary.worktree_id.clone();
        state
            .entries
            .insert(summary.worktree_id.clone(), summary.clone());
        self.apply_active_context(&summary);
        self.states
            .lock()
            .expect("worktree state lock")
            .insert(session_id, state);
        self.publish_entered(&runtime, summary.clone()).await?;
        Ok(summary)
    }

    async fn list_worktrees(
        &self,
        runtime: WorktreeRuntimeContext,
        include_inactive: bool,
    ) -> ToolResult<Vec<WorktreeSummaryRecord>> {
        let state = self.load_or_restore_state(&runtime).await?;
        let mut worktrees = state.entries.into_values().collect::<Vec<_>>();
        worktrees.sort_by(|left, right| {
            right
                .created_at_unix_s
                .cmp(&left.created_at_unix_s)
                .then_with(|| left.worktree_id.cmp(&right.worktree_id))
        });
        if include_inactive {
            Ok(worktrees)
        } else {
            Ok(worktrees
                .into_iter()
                .filter(|summary| summary.status.is_active())
                .collect())
        }
    }

    async fn exit_worktree(
        &self,
        runtime: WorktreeRuntimeContext,
        worktree_id: Option<WorktreeId>,
    ) -> ToolResult<WorktreeSummaryRecord> {
        let (session_id, _) = Self::require_attached_runtime(&runtime)?;
        let mut state = self.load_or_restore_state(&runtime).await?;
        let target_id = worktree_id.unwrap_or_else(|| state.current_worktree_id.clone());
        let summary = state
            .entries
            .get(&target_id)
            .cloned()
            .ok_or_else(|| ToolError::invalid(format!("unknown worktree id: {target_id}")))?;
        self.remove_session_worktree(&runtime, &mut state, &summary)
            .await?;
        self.states
            .lock()
            .expect("worktree state lock")
            .insert(session_id, state);
        Ok(summary)
    }

    async fn create_child_worktree(
        &self,
        runtime: WorktreeRuntimeContext,
        request: ChildWorktreeRequest,
    ) -> ToolResult<WorktreeSummaryRecord> {
        let (session_id, _) = Self::require_attached_runtime(&runtime)?;
        let mut state = self.load_or_restore_state(&runtime).await?;
        let root = self
            .create_git_worktree(request.label.as_deref(), "child")
            .await?;
        let summary = WorktreeSummaryRecord {
            worktree_id: WorktreeId::from(format!("worktree_{}", new_opaque_id())),
            session_id: request.child_session_id,
            agent_session_id: request.child_agent_session_id,
            scope: WorktreeScope::ChildAgent,
            status: WorktreeStatus::Active,
            root,
            parent_agent_id: runtime.parent_agent_id.clone(),
            task_id: Some(request.task_id),
            child_agent_id: Some(request.child_agent_id),
            label: Self::normalized_label(request.label),
            created_at_unix_s: unix_timestamp_s(),
            updated_at_unix_s: None,
        };
        state
            .entries
            .insert(summary.worktree_id.clone(), summary.clone());
        self.states
            .lock()
            .expect("worktree state lock")
            .insert(session_id, state);
        self.publish_entered(&runtime, summary.clone()).await?;
        Ok(summary)
    }

    async fn release_child_worktree(
        &self,
        runtime: WorktreeRuntimeContext,
        worktree_id: &WorktreeId,
    ) -> ToolResult<Option<WorktreeSummaryRecord>> {
        let (session_id, _) = Self::require_attached_runtime(&runtime)?;
        let mut state = self.load_or_restore_state(&runtime).await?;
        let Some(summary) = state.entries.get(worktree_id).cloned() else {
            return Ok(None);
        };
        if summary.scope != WorktreeScope::ChildAgent {
            return Err(ToolError::invalid(format!(
                "worktree {worktree_id} is not a child-agent worktree"
            )));
        }
        if summary.status == WorktreeStatus::Removed {
            return Ok(Some(summary));
        }
        let root_string = summary.root.to_string_lossy().to_string();
        self.run_git(
            &self.primary_root,
            &["worktree", "remove", "--force", &root_string],
        )
        .await?;
        let mut removed = summary;
        removed.status = WorktreeStatus::Removed;
        removed.updated_at_unix_s = Some(unix_timestamp_s());
        state
            .entries
            .insert(removed.worktree_id.clone(), removed.clone());
        self.states
            .lock()
            .expect("worktree state lock")
            .insert(session_id, state);
        self.publish_updated(&runtime, removed.clone()).await?;
        Ok(Some(removed))
    }
}

fn unix_timestamp_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn sanitize_worktree_label(label: &str) -> String {
    let trimmed = label.trim();
    let sanitized = trimmed
        .chars()
        .map(|ch| match ch {
            'a'..='z' | '0'..='9' => ch,
            'A'..='Z' => ch.to_ascii_lowercase(),
            _ => '-',
        })
        .collect::<String>();
    let collapsed = sanitized
        .split('-')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if collapsed.is_empty() {
        "session".to_string()
    } else {
        collapsed
    }
}

#[cfg(test)]
mod tests {
    use super::SessionWorktreeManager;
    use crate::backend::SessionEventStream;
    use agent::ManagedPolicyProcessExecutor;
    use agent::tools::{
        ChildWorktreeRequest, PRIMARY_WORKTREE_ID, ToolExecutionContext, WorktreeEnterRequest,
        WorktreeManager, WorktreeRuntimeContext,
    };
    use agent::types::{
        AgentId, AgentSessionId, SessionEventKind, SessionId, TaskId, WorktreeId, WorktreeScope,
        WorktreeStatus,
    };
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use std::sync::{Arc, RwLock};
    use store::{InMemorySessionStore, SessionStore};

    #[tokio::test]
    async fn enter_and_exit_worktree_updates_context_and_persists_events() {
        let repo = tempfile::tempdir().unwrap();
        init_git_repo(repo.path());
        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let tool_context = Arc::new(RwLock::new(ToolExecutionContext {
            workspace_root: repo.path().to_path_buf(),
            worktree_root: Some(repo.path().to_path_buf()),
            active_worktree_id: Some(WorktreeId::from(PRIMARY_WORKTREE_ID)),
            ..Default::default()
        }));
        let manager = SessionWorktreeManager::new(
            store.clone(),
            SessionEventStream::default(),
            Arc::new(ManagedPolicyProcessExecutor::new()),
            repo.path().to_path_buf(),
            tool_context.clone(),
        );
        let runtime = WorktreeRuntimeContext {
            session_id: Some(SessionId::from("session_root")),
            agent_session_id: Some(AgentSessionId::from("agent_session_root")),
            ..Default::default()
        };

        let entered = manager
            .enter_worktree(
                runtime.clone(),
                WorktreeEnterRequest {
                    label: Some("feature auth".to_string()),
                },
            )
            .await
            .unwrap();

        assert_eq!(entered.status, WorktreeStatus::Active);
        assert!(entered.root.exists());
        let active_context = tool_context.read().unwrap().clone();
        assert_eq!(
            active_context.active_worktree_id,
            Some(entered.worktree_id.clone())
        );
        assert_eq!(active_context.workspace_root, entered.root);

        let listed = manager.list_worktrees(runtime.clone(), true).await.unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().any(|summary| {
            summary.worktree_id == WorktreeId::from(PRIMARY_WORKTREE_ID)
                && summary.status == WorktreeStatus::Inactive
        }));

        manager.exit_worktree(runtime.clone(), None).await.unwrap();

        let restored_context = tool_context.read().unwrap().clone();
        assert_eq!(
            restored_context.active_worktree_id,
            Some(WorktreeId::from(PRIMARY_WORKTREE_ID))
        );
        assert_eq!(restored_context.workspace_root, repo.path().to_path_buf());

        let events = store
            .events(&SessionId::from("session_root"))
            .await
            .unwrap();
        assert!(
            events
                .iter()
                .any(|event| matches!(event.event, SessionEventKind::WorktreeEntered { .. }))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event.event, SessionEventKind::WorktreeUpdated { .. }))
        );
    }

    #[tokio::test]
    async fn sync_attached_session_restores_active_worktree_from_store() {
        let repo = tempfile::tempdir().unwrap();
        init_git_repo(repo.path());
        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let initial_context = Arc::new(RwLock::new(ToolExecutionContext {
            workspace_root: repo.path().to_path_buf(),
            worktree_root: Some(repo.path().to_path_buf()),
            active_worktree_id: Some(WorktreeId::from(PRIMARY_WORKTREE_ID)),
            ..Default::default()
        }));
        let runtime = WorktreeRuntimeContext {
            session_id: Some(SessionId::from("session_restore")),
            agent_session_id: Some(AgentSessionId::from("agent_session_restore")),
            ..Default::default()
        };
        let manager = SessionWorktreeManager::new(
            store.clone(),
            SessionEventStream::default(),
            Arc::new(ManagedPolicyProcessExecutor::new()),
            repo.path().to_path_buf(),
            initial_context,
        );
        let entered = manager
            .enter_worktree(
                runtime,
                WorktreeEnterRequest {
                    label: Some("restore me".to_string()),
                },
            )
            .await
            .unwrap();

        let restored_context = Arc::new(RwLock::new(ToolExecutionContext {
            workspace_root: repo.path().to_path_buf(),
            worktree_root: Some(repo.path().to_path_buf()),
            active_worktree_id: Some(WorktreeId::from(PRIMARY_WORKTREE_ID)),
            ..Default::default()
        }));
        let restored_manager = SessionWorktreeManager::new(
            store,
            SessionEventStream::default(),
            Arc::new(ManagedPolicyProcessExecutor::new()),
            repo.path().to_path_buf(),
            restored_context.clone(),
        );

        restored_manager
            .sync_attached_session(
                SessionId::from("session_restore"),
                AgentSessionId::from("agent_session_restore"),
            )
            .await
            .unwrap();

        let restored = restored_context.read().unwrap().clone();
        assert_eq!(restored.active_worktree_id, Some(entered.worktree_id));
        assert_eq!(restored.workspace_root, entered.root);
    }

    #[tokio::test]
    async fn child_worktrees_are_tracked_without_switching_session_context() {
        let repo = tempfile::tempdir().unwrap();
        init_git_repo(repo.path());
        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let tool_context = Arc::new(RwLock::new(ToolExecutionContext {
            workspace_root: repo.path().to_path_buf(),
            worktree_root: Some(repo.path().to_path_buf()),
            active_worktree_id: Some(WorktreeId::from(PRIMARY_WORKTREE_ID)),
            ..Default::default()
        }));
        let manager = SessionWorktreeManager::new(
            store.clone(),
            SessionEventStream::default(),
            Arc::new(ManagedPolicyProcessExecutor::new()),
            repo.path().to_path_buf(),
            tool_context.clone(),
        );
        let runtime = WorktreeRuntimeContext {
            session_id: Some(SessionId::from("session_root")),
            agent_session_id: Some(AgentSessionId::from("agent_session_root")),
            parent_agent_id: Some(AgentId::from("agent_parent")),
            ..Default::default()
        };

        let child = manager
            .create_child_worktree(
                runtime.clone(),
                ChildWorktreeRequest {
                    child_agent_id: AgentId::from("agent_child"),
                    child_session_id: SessionId::from("session_child"),
                    child_agent_session_id: AgentSessionId::from("agent_session_child"),
                    task_id: TaskId::from("task_child"),
                    label: Some("reviewer".to_string()),
                },
            )
            .await
            .unwrap();

        assert_eq!(child.scope, WorktreeScope::ChildAgent);
        assert_eq!(child.child_agent_id, Some(AgentId::from("agent_child")));
        assert_eq!(
            tool_context.read().unwrap().workspace_root,
            repo.path().to_path_buf()
        );

        let listed = manager.list_worktrees(runtime.clone(), true).await.unwrap();
        assert!(
            listed
                .iter()
                .any(|summary| summary.worktree_id == child.worktree_id)
        );

        let released = manager
            .release_child_worktree(runtime.clone(), &child.worktree_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(released.status, WorktreeStatus::Removed);
        assert_eq!(
            tool_context.read().unwrap().workspace_root,
            repo.path().to_path_buf()
        );

        let events = store
            .events(&SessionId::from("session_root"))
            .await
            .unwrap();
        assert!(events.iter().any(|event| {
            matches!(
                &event.event,
                SessionEventKind::WorktreeEntered { summary }
                    if summary.worktree_id == child.worktree_id
            )
        }));
        assert!(events.iter().any(|event| {
            matches!(
                &event.event,
                SessionEventKind::WorktreeUpdated { summary }
                    if summary.worktree_id == child.worktree_id
                        && summary.status == WorktreeStatus::Removed
            )
        }));
    }

    fn init_git_repo(path: &Path) {
        run_git(path, &["init"]);
        run_git(path, &["config", "user.email", "nanoclaw@example.test"]);
        run_git(path, &["config", "user.name", "Nanoclaw Tests"]);
        fs::write(path.join("README.md"), "worktree test\n").unwrap();
        run_git(path, &["add", "README.md"]);
        run_git(path, &["commit", "-m", "initial"]);
    }

    fn run_git(path: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
