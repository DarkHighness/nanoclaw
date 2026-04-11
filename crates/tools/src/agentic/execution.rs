use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::registry::Tool;
use crate::{Result, ToolError, ToolExecutionContext};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use types::{MessagePart, ToolCallId, ToolOutputMode, ToolResult, ToolSpec};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionAction {
    #[default]
    Set,
    Get,
    Clear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    // Models sometimes reuse the plan vocabulary when updating execution
    // state. Treating `in_progress` as `active` keeps the runtime resilient
    // without widening the canonical status we emit back to the UI.
    #[serde(alias = "in_progress")]
    Active,
    Blocked,
    Verifying,
    Completed,
}

impl ExecutionStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Blocked => "blocked",
            Self::Verifying => "verifying",
            Self::Completed => "completed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionSnapshot {
    pub status: ExecutionStatus,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocker: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionScopeDescriptor {
    pub scope_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub label: String,
}

#[derive(Clone, Debug, Default)]
pub struct ExecutionState {
    snapshots: Arc<Mutex<BTreeMap<String, ScopedExecutionState>>>,
}

impl ExecutionState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn snapshot(&self, scope: &ExecutionScopeDescriptor) -> Option<ScopedExecutionState> {
        self.snapshots
            .lock()
            .expect("execution state lock")
            .get(&scope.scope_key)
            .cloned()
    }

    pub async fn set(&self, scope: ExecutionScopeDescriptor, snapshot: ExecutionSnapshot) {
        self.snapshots.lock().expect("execution state lock").insert(
            scope.scope_key.clone(),
            ScopedExecutionState { scope, snapshot },
        );
    }

    pub async fn clear(&self, scope: &ExecutionScopeDescriptor) -> Option<ScopedExecutionState> {
        self.snapshots
            .lock()
            .expect("execution state lock")
            .remove(&scope.scope_key)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopedExecutionState {
    pub scope: ExecutionScopeDescriptor,
    pub snapshot: ExecutionSnapshot,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct UpdateExecutionInput {
    #[serde(default)]
    pub action: ExecutionAction,
    #[serde(default)]
    pub status: Option<ExecutionStatus>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub next_action: Option<String>,
    #[serde(default)]
    pub verification: Option<String>,
    #[serde(default)]
    pub blocker: Option<String>,
    #[serde(default)]
    pub expected_revision: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum UpdateExecutionToolOutput {
    Success {
        action: ExecutionAction,
        scope: ExecutionScopeDescriptor,
        revision_before: String,
        revision_after: String,
        state: Option<ExecutionSnapshot>,
    },
    Error {
        action: ExecutionAction,
        scope: ExecutionScopeDescriptor,
        expected_revision: String,
        revision_before: String,
        state: Option<ExecutionSnapshot>,
    },
}

#[derive(Clone, Debug)]
pub struct UpdateExecutionTool {
    state: ExecutionState,
}

impl UpdateExecutionTool {
    #[must_use]
    pub fn new(state: ExecutionState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Tool for UpdateExecutionTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "update_execution",
            "Track the live execution state for the current session or subagent. Use this to record the actively executing slice, blockers, and verification status. Status must be one of active, blocked, verifying, or completed; do not use plan labels such as in_progress here. Do not mirror the full task plan here.",
            serde_json::to_value(schema_for!(UpdateExecutionInput))
                .expect("update_execution schema"),
            ToolOutputMode::Text,
            // Execution snapshots are internal coordination state. They should
            // not ask for approval like filesystem or process side effects.
            tool_approval_profile(false, false, true, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(UpdateExecutionToolOutput))
                .expect("update_execution output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: UpdateExecutionInput = serde_json::from_value(arguments)?;
        let scope = execution_scope_from_context(ctx);
        let current = self.state.snapshot(&scope).await;
        let current_snapshot = current.as_ref().map(|entry| entry.snapshot.clone());
        let revision_before = revision_for(current_snapshot.as_ref());
        let action = input.action;

        if let Some(expected_revision) = input.expected_revision.as_deref()
            && expected_revision != revision_before
        {
            let structured_output = UpdateExecutionToolOutput::Error {
                action,
                scope: scope.clone(),
                expected_revision: expected_revision.to_string(),
                revision_before: revision_before.clone(),
                state: current_snapshot.clone(),
            };
            return Ok(ToolResult {
                id: call_id,
                call_id: external_call_id,
                tool_name: "update_execution".into(),
                parts: vec![MessagePart::text(format!(
                    "Execution revision mismatch. Expected {expected_revision}, found {revision_before}. Refresh the current execution state before updating it."
                ))],
                attachments: Vec::new(),
                structured_content: Some(
                    serde_json::to_value(structured_output).expect("update_execution error output"),
                ),
                continuation: None,
                metadata: Some(serde_json::json!({
                    "scope": scope,
                    "expected_revision": expected_revision,
                    "revision_before": revision_before,
                    "state": current_snapshot,
                })),
                is_error: true,
            });
        }

        let next_snapshot = match action {
            ExecutionAction::Set => {
                let snapshot = normalize_execution_snapshot(&input)?;
                self.state.set(scope.clone(), snapshot.clone()).await;
                Some(snapshot)
            }
            ExecutionAction::Get => current_snapshot.clone(),
            ExecutionAction::Clear => {
                self.state.clear(&scope).await;
                None
            }
        };
        let revision_after = revision_for(next_snapshot.as_ref());
        let structured_output = UpdateExecutionToolOutput::Success {
            action,
            scope: scope.clone(),
            revision_before: revision_before.clone(),
            revision_after: revision_after.clone(),
            state: next_snapshot.clone(),
        };

        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "update_execution".into(),
            parts: vec![MessagePart::text(render_execution_update(
                action,
                &scope,
                &revision_before,
                &revision_after,
                next_snapshot.as_ref(),
            ))],
            attachments: Vec::new(),
            structured_content: Some(
                serde_json::to_value(structured_output).expect("update_execution success output"),
            ),
            continuation: None,
            metadata: Some(serde_json::json!({
                "action": action,
                "scope": scope,
                "revision_before": revision_before,
                "revision_after": revision_after,
                "state": next_snapshot,
            })),
            is_error: false,
        })
    }
}

fn normalize_execution_snapshot(input: &UpdateExecutionInput) -> Result<ExecutionSnapshot> {
    let summary = input
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ToolError::invalid("update_execution set requires a non-empty summary"))?;
    let status = input
        .status
        .ok_or_else(|| ToolError::invalid("update_execution set requires an explicit status"))?;
    let blocker = normalize_optional(input.blocker.clone());
    if matches!(status, ExecutionStatus::Blocked) && blocker.is_none() {
        return Err(ToolError::invalid(
            "blocked execution state requires a blocker",
        ));
    }
    Ok(ExecutionSnapshot {
        status,
        summary: summary.to_string(),
        next_action: normalize_optional(input.next_action.clone()),
        verification: normalize_optional(input.verification.clone()),
        blocker,
    })
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn render_execution_update(
    action: ExecutionAction,
    scope: &ExecutionScopeDescriptor,
    revision_before: &str,
    revision_after: &str,
    snapshot: Option<&ExecutionSnapshot>,
) -> String {
    let mut lines = vec![format!(
        "[execution action={} scope={} revision {} -> {}]",
        action_label(action),
        scope.label,
        revision_before,
        revision_after
    )];

    match snapshot {
        Some(snapshot) => {
            lines.push(format!("status> {}", snapshot.status.as_str()));
            lines.push(format!("summary> {}", snapshot.summary));
            if let Some(next_action) = snapshot.next_action.as_deref() {
                lines.push(format!("next> {next_action}"));
            }
            if let Some(verification) = snapshot.verification.as_deref() {
                lines.push(format!("verify> {verification}"));
            }
            if let Some(blocker) = snapshot.blocker.as_deref() {
                lines.push(format!("blocker> {blocker}"));
            }
        }
        None if matches!(action, ExecutionAction::Clear) => {
            lines.push("Execution state cleared.".to_string());
        }
        None => {
            lines.push("No execution state recorded for this scope.".to_string());
        }
    }

    lines.join("\n")
}

fn action_label(action: ExecutionAction) -> &'static str {
    match action {
        ExecutionAction::Set => "set",
        ExecutionAction::Get => "get",
        ExecutionAction::Clear => "clear",
    }
}

fn execution_scope_from_context(ctx: &ToolExecutionContext) -> ExecutionScopeDescriptor {
    let session_id = ctx.session_id.as_ref().map(ToString::to_string);
    let agent_session_id = ctx.agent_session_id.as_ref().map(ToString::to_string);
    let agent_id = ctx.agent_id.as_ref().map(ToString::to_string);
    let agent_name = ctx
        .agent_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let task_id = ctx
        .task_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let scope_key = agent_id
        .clone()
        .map(|value| format!("agent:{value}"))
        .or_else(|| {
            agent_session_id
                .clone()
                .map(|value| format!("agent_session:{value}"))
        })
        .or_else(|| session_id.clone().map(|value| format!("session:{value}")))
        .unwrap_or_else(|| format!("workspace:{}", ctx.workspace_root.display()));
    let label = agent_name
        .clone()
        .or_else(|| task_id.clone().map(|value| format!("task {value}")))
        .or_else(|| agent_id.clone().map(|value| format!("agent {value}")))
        .or_else(|| {
            agent_session_id
                .clone()
                .map(|value| format!("agent session {value}"))
        })
        .or_else(|| session_id.clone().map(|value| format!("session {value}")))
        .unwrap_or_else(|| "workspace".to_string());

    ExecutionScopeDescriptor {
        scope_key,
        session_id,
        agent_session_id,
        agent_id,
        agent_name,
        task_id,
        label,
    }
}

fn revision_for(snapshot: Option<&ExecutionSnapshot>) -> String {
    crate::stable_text_hash(&serde_json::to_string(&snapshot).expect("execution revision json"))
}

#[cfg(test)]
mod tests {
    use super::{
        ExecutionAction, ExecutionState, ExecutionStatus, UpdateExecutionTool,
        execution_scope_from_context,
    };
    use crate::{Tool, ToolExecutionContext};
    use serde_json::json;
    use tempfile::tempdir;
    use types::{AgentId, AgentSessionId, SessionId, ToolCallId};

    #[tokio::test]
    async fn execution_state_is_scoped_by_runtime_context() {
        let dir = tempdir().unwrap();
        let state = ExecutionState::new();
        let tool = UpdateExecutionTool::new(state.clone());
        let ctx = ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            session_id: Some(SessionId::from("session-root")),
            agent_session_id: Some(AgentSessionId::from("agent-root")),
            agent_id: Some(AgentId::from("agent-1")),
            agent_name: Some("review_worker".to_string()),
            task_id: Some("review-runtime".to_string()),
            ..Default::default()
        };

        let result = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "status": "active",
                    "summary": "Wire the execution side rail",
                    "next_action": "Patch the TUI observer",
                    "verification": "observer test pending"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.text_content().contains("scope=review_worker"));
        let scope = execution_scope_from_context(&ctx);
        let snapshot = state.snapshot(&scope).await.expect("execution snapshot");
        assert_eq!(snapshot.scope.agent_id.as_deref(), Some("agent-1"));
        assert_eq!(snapshot.snapshot.status, ExecutionStatus::Active);
        assert_eq!(snapshot.snapshot.summary, "Wire the execution side rail");
    }

    #[tokio::test]
    async fn blocked_execution_requires_blocker() {
        let tool = UpdateExecutionTool::new(ExecutionState::new());
        let error = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "status": "blocked",
                    "summary": "Waiting for host runtime wiring"
                }),
                &ToolExecutionContext::default(),
            )
            .await
            .expect_err("blocked execution without blocker should fail");
        assert!(error.to_string().contains("requires a blocker"));
    }

    #[tokio::test]
    async fn get_and_clear_preserve_revision_contract() {
        let tool = UpdateExecutionTool::new(ExecutionState::new());
        let ctx = ToolExecutionContext {
            session_id: Some(SessionId::from("session-root")),
            ..Default::default()
        };
        let set = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "status": "verifying",
                    "summary": "Running regression checks",
                    "verification": "cargo test -p code-agent"
                }),
                &ctx,
            )
            .await
            .unwrap();
        let revision = set.structured_content.as_ref().unwrap()["revision_after"]
            .as_str()
            .unwrap()
            .to_string();

        let get = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "action": "get"
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(
            get.structured_content.as_ref().unwrap()["state"]["status"],
            "verifying"
        );

        let clear = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "action": "clear",
                    "expected_revision": revision
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert_eq!(
            clear.structured_content.as_ref().unwrap()["state"],
            serde_json::Value::Null
        );
        assert!(clear.text_content().contains("Execution state cleared"));
    }

    #[tokio::test]
    async fn stale_revision_returns_structured_error() {
        let tool = UpdateExecutionTool::new(ExecutionState::new());
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "status": "completed",
                    "summary": "Finished the current slice"
                }),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);

        let stale = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "action": "clear",
                    "expected_revision": "stale"
                }),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert!(stale.is_error);
        assert_eq!(stale.structured_content.as_ref().unwrap()["kind"], "error");
        assert_eq!(
            stale.structured_content.as_ref().unwrap()["expected_revision"],
            "stale"
        );
    }

    #[test]
    fn action_defaults_to_set() {
        let parsed = serde_json::from_value::<super::UpdateExecutionInput>(json!({
            "status": "active",
            "summary": "Inspect runtime"
        }))
        .unwrap();
        assert_eq!(parsed.action, ExecutionAction::Set);
    }

    #[test]
    fn in_progress_status_alias_maps_to_active() {
        let parsed = serde_json::from_value::<super::UpdateExecutionInput>(json!({
            "status": "in_progress",
            "summary": "Inspect runtime"
        }))
        .unwrap();
        assert_eq!(parsed.status, Some(ExecutionStatus::Active));
    }

    #[test]
    fn update_execution_spec_is_approval_free_for_internal_coordination() {
        let spec = UpdateExecutionTool::new(ExecutionState::new()).spec();
        assert!(!spec.approval.mutates_state);
        assert!(!spec.approval.open_world);
        assert_eq!(spec.approval.idempotent, Some(true));
    }

    #[test]
    fn update_execution_spec_publishes_canonical_status_values() {
        let spec = UpdateExecutionTool::new(ExecutionState::new()).spec();
        let schema = spec
            .input_schema
            .as_ref()
            .expect("update_execution input schema")
            .to_string();

        assert!(
            spec.description
                .contains("active, blocked, verifying, or completed")
        );
        assert!(
            spec.description
                .contains("do not use plan labels such as in_progress")
        );
        assert!(schema.contains("\"active\""));
        assert!(schema.contains("\"blocked\""));
        assert!(schema.contains("\"verifying\""));
        assert!(schema.contains("\"completed\""));
        assert!(!schema.contains("\"in_progress\""));
    }
}
