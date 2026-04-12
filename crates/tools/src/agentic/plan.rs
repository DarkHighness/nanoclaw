use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::registry::Tool;
use crate::{Result, ToolError, ToolExecutionContext};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use types::{MessagePart, ToolCallId, ToolOutputMode, ToolResult, ToolSpec};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanItem {
    pub step: String,
    pub status: PlanStatus,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanFocusAction {
    #[default]
    Set,
    Clear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanFocusStatus {
    // Models still occasionally reuse the plan vocabulary when describing the
    // live focus. Accepting `in_progress` keeps the canonical schema strict
    // while staying resilient to adjacent tool vocabulary.
    #[serde(alias = "in_progress")]
    Active,
    Blocked,
    Verifying,
    Completed,
}

impl PlanFocusStatus {
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
pub struct PlanFocusSnapshot {
    pub scope_label: String,
    pub status: PlanFocusStatus,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocker: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanSnapshot {
    #[serde(default)]
    pub items: Vec<PlanItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<PlanFocusSnapshot>,
}

#[derive(Clone, Debug, Default)]
pub struct PlanState {
    snapshot: Arc<Mutex<PlanSnapshot>>,
}

impl PlanState {
    #[must_use]
    pub fn new(initial_items: Vec<PlanItem>) -> Self {
        Self {
            snapshot: Arc::new(Mutex::new(PlanSnapshot {
                items: initial_items,
                focus: None,
            })),
        }
    }

    #[must_use]
    pub fn new_with_snapshot(snapshot: PlanSnapshot) -> Self {
        Self {
            snapshot: Arc::new(Mutex::new(snapshot)),
        }
    }

    pub async fn snapshot(&self) -> PlanSnapshot {
        self.snapshot.lock().expect("plan state lock").clone()
    }

    pub async fn replace(&self, snapshot: PlanSnapshot) {
        *self.snapshot.lock().expect("plan state lock") = snapshot;
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct PlanFocusInput {
    #[serde(default)]
    pub action: PlanFocusAction,
    #[serde(default)]
    pub status: Option<PlanFocusStatus>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub next_action: Option<String>,
    #[serde(default)]
    pub verification: Option<String>,
    #[serde(default)]
    pub blocker: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct UpdatePlanInput {
    #[serde(default)]
    pub explanation: Option<String>,
    #[serde(default)]
    pub plan: Option<Vec<PlanItem>>,
    #[serde(default)]
    pub focus: Option<PlanFocusInput>,
    #[serde(default)]
    pub expected_revision: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum UpdatePlanToolOutput {
    Success {
        explanation: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        warnings: Vec<String>,
        count: usize,
        plan_updated: bool,
        focus_updated: bool,
        revision_before: String,
        revision_after: String,
        pending_count: usize,
        in_progress_count: usize,
        completed_count: usize,
        items: Vec<PlanItem>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        focus: Option<PlanFocusSnapshot>,
    },
    Error {
        explanation: Option<String>,
        expected_revision: String,
        revision_before: String,
        items: Vec<PlanItem>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        focus: Option<PlanFocusSnapshot>,
    },
}

#[derive(Clone, Debug)]
pub struct UpdatePlanTool {
    state: PlanState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NormalizedPlan {
    items: Vec<PlanItem>,
    warnings: Vec<String>,
}

impl UpdatePlanTool {
    #[must_use]
    pub fn new(state: PlanState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Tool for UpdatePlanTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "update_plan",
            "Maintain the shared task plan and optional live focus state. Use plan to replace the full ordered plan when it changes. Use focus to record the actively executing slice, blockers, and verification status, or clear it when no focused slice should be shown. Omit plan or focus to keep the current value unchanged.",
            serde_json::to_value(schema_for!(UpdatePlanInput)).expect("update_plan schema"),
            ToolOutputMode::Text,
            // This mutates host-owned workflow state, not the workspace or any
            // external system, so it should stay outside the approval path.
            tool_approval_profile(false, false, true, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(UpdatePlanToolOutput))
                .expect("update_plan output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: UpdatePlanInput = serde_json::from_value(arguments)?;
        if input.plan.is_none() && input.focus.is_none() {
            return Err(ToolError::invalid(
                "update_plan requires `plan` and/or `focus`",
            ));
        }

        let before = self.state.snapshot().await;
        let revision_before = revision_for(&before);
        if let Some(expected_revision) = input.expected_revision.as_deref()
            && expected_revision != revision_before
        {
            let text = format!(
                "Plan revision mismatch. Expected {expected_revision}, found {revision_before}. Re-read the current plan state before updating it."
            );
            let structured_output = UpdatePlanToolOutput::Error {
                explanation: input.explanation.clone(),
                expected_revision: expected_revision.to_string(),
                revision_before: revision_before.clone(),
                items: before.items.clone(),
                focus: before.focus.clone(),
            };
            return Ok(ToolResult {
                id: call_id,
                call_id: external_call_id,
                tool_name: "update_plan".into(),
                parts: vec![MessagePart::text(text)],
                attachments: Vec::new(),
                structured_content: Some(
                    serde_json::to_value(structured_output)
                        .expect("update_plan error structured output"),
                ),
                continuation: None,
                metadata: Some(serde_json::json!({
                    "expected_revision": expected_revision,
                    "revision_before": revision_before,
                    "items": before.items,
                    "focus": before.focus,
                })),
                is_error: true,
            });
        }

        let normalized = input
            .plan
            .clone()
            .map(normalize_plan_items)
            .transpose()?
            .unwrap_or_else(|| NormalizedPlan {
                items: before.items.clone(),
                warnings: Vec::new(),
            });
        let next_focus = match input.focus.as_ref() {
            Some(focus) => normalize_focus_update(focus, ctx)?,
            None => before.focus.clone(),
        };
        let next = PlanSnapshot {
            items: normalized.items.clone(),
            focus: next_focus.clone(),
        };
        self.state.replace(next.clone()).await;

        let revision_after = revision_for(&next);
        let (pending_count, in_progress_count, completed_count) = status_counts(&next.items);
        let structured_output = UpdatePlanToolOutput::Success {
            explanation: input.explanation.clone(),
            warnings: normalized.warnings.clone(),
            count: next.items.len(),
            plan_updated: input.plan.is_some(),
            focus_updated: input.focus.is_some(),
            revision_before: revision_before.clone(),
            revision_after: revision_after.clone(),
            pending_count,
            in_progress_count,
            completed_count,
            items: next.items.clone(),
            focus: next.focus.clone(),
        };

        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "update_plan".into(),
            parts: vec![MessagePart::text(render_plan_update(
                input.explanation.as_deref(),
                &normalized.warnings,
                input.plan.is_some(),
                input.focus.is_some(),
                &next,
                &revision_before,
                &revision_after,
            ))],
            attachments: Vec::new(),
            structured_content: Some(
                serde_json::to_value(structured_output).expect("update_plan structured output"),
            ),
            continuation: None,
            metadata: Some(serde_json::json!({
                "count": next.items.len(),
                "plan_updated": input.plan.is_some(),
                "focus_updated": input.focus.is_some(),
                "revision_before": revision_before,
                "revision_after": revision_after,
                "pending_count": pending_count,
                "in_progress_count": in_progress_count,
                "completed_count": completed_count,
                "warnings": normalized.warnings,
                "items": next.items,
                "focus": next.focus,
            })),
            is_error: false,
        })
    }
}

fn normalize_plan_items(items: Vec<PlanItem>) -> Result<NormalizedPlan> {
    let mut normalized = Vec::with_capacity(items.len());
    let mut saw_in_progress = false;
    let mut demoted_in_progress_count = 0usize;
    for item in items {
        let step = item.step.trim();
        if step.is_empty() {
            return Err(ToolError::invalid(
                "update_plan requires every step to have non-empty text",
            ));
        }
        let status = if matches!(item.status, PlanStatus::InProgress) {
            if saw_in_progress {
                demoted_in_progress_count += 1;
                PlanStatus::Pending
            } else {
                saw_in_progress = true;
                PlanStatus::InProgress
            }
        } else {
            item.status
        };
        normalized.push(PlanItem {
            step: step.to_string(),
            status,
        });
    }

    let mut warnings = Vec::new();
    if demoted_in_progress_count > 0 {
        // Agents often resend the whole plan and accidentally mark multiple
        // steps active. Empty steps and revision mismatches still fail closed,
        // but duplicate in-progress states are safe to normalize locally.
        warnings.push(format!(
            "demoted {demoted_in_progress_count} extra in_progress step(s) to pending so the plan keeps a single active item"
        ));
    }

    Ok(NormalizedPlan {
        items: normalized,
        warnings,
    })
}

fn normalize_focus_update(
    focus: &PlanFocusInput,
    ctx: &ToolExecutionContext,
) -> Result<Option<PlanFocusSnapshot>> {
    match focus.action {
        PlanFocusAction::Clear => Ok(None),
        PlanFocusAction::Set => {
            let summary = normalize_optional(focus.summary.clone()).ok_or_else(|| {
                ToolError::invalid("update_plan focus set requires a non-empty summary")
            })?;
            let status = focus.status.ok_or_else(|| {
                ToolError::invalid("update_plan focus set requires an explicit status")
            })?;
            let blocker = normalize_optional(focus.blocker.clone());
            if matches!(status, PlanFocusStatus::Blocked) && blocker.is_none() {
                return Err(ToolError::invalid(
                    "update_plan blocked focus requires a blocker",
                ));
            }
            Ok(Some(PlanFocusSnapshot {
                scope_label: focus_scope_label_from_context(ctx),
                status,
                summary,
                next_action: normalize_optional(focus.next_action.clone()),
                verification: normalize_optional(focus.verification.clone()),
                blocker,
            }))
        }
    }
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn focus_scope_label_from_context(ctx: &ToolExecutionContext) -> String {
    ctx.agent_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            ctx.task_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| format!("task {value}"))
        })
        .or_else(|| ctx.agent_id.as_ref().map(|value| format!("agent {value}")))
        .or_else(|| {
            ctx.agent_session_id
                .as_ref()
                .map(|value| format!("agent session {value}"))
        })
        .or_else(|| {
            ctx.session_id
                .as_ref()
                .map(|value| format!("session {value}"))
        })
        .unwrap_or_else(|| "workspace".to_string())
}

fn render_plan_update(
    explanation: Option<&str>,
    warnings: &[String],
    plan_updated: bool,
    focus_updated: bool,
    snapshot: &PlanSnapshot,
    revision_before: &str,
    revision_after: &str,
) -> String {
    let mut sections = vec![format!(
        "[plan count={} revision {} -> {}]",
        snapshot.items.len(),
        revision_before,
        revision_after
    )];
    if let Some(explanation) = explanation.map(str::trim).filter(|value| !value.is_empty()) {
        sections.push(format!("explanation> {explanation}"));
    }
    sections.extend(warnings.iter().map(|warning| format!("warning> {warning}")));
    if focus_updated {
        match snapshot.focus.as_ref() {
            Some(focus) => {
                sections.push(format!(
                    "focus> [{}] {}",
                    focus.status.as_str(),
                    focus.summary
                ));
                sections.push(format!("scope> {}", focus.scope_label));
                if let Some(next_action) = focus.next_action.as_deref() {
                    sections.push(format!("next_action> {next_action}"));
                }
                if let Some(verification) = focus.verification.as_deref() {
                    sections.push(format!("verification> {verification}"));
                }
                if let Some(blocker) = focus.blocker.as_deref() {
                    sections.push(format!("blocker> {blocker}"));
                }
            }
            None => sections.push("focus> cleared".to_string()),
        }
    }
    if plan_updated {
        sections.push(String::new());
        sections.push(render_plan(&snapshot.items));
    }
    sections.join("\n")
}

fn render_plan(items: &[PlanItem]) -> String {
    if items.is_empty() {
        return "Plan cleared.".to_string();
    }
    items
        .iter()
        .map(|item| format!("- [{}] {}", status_marker(&item.status), item.step))
        .collect::<Vec<_>>()
        .join("\n")
}

fn status_marker(status: &PlanStatus) -> &'static str {
    match status {
        PlanStatus::Pending => " ",
        PlanStatus::InProgress => "~",
        PlanStatus::Completed => "x",
    }
}

fn status_counts(items: &[PlanItem]) -> (usize, usize, usize) {
    let mut pending = 0usize;
    let mut in_progress = 0usize;
    let mut completed = 0usize;
    for item in items {
        match item.status {
            PlanStatus::Pending => pending += 1,
            PlanStatus::InProgress => in_progress += 1,
            PlanStatus::Completed => completed += 1,
        }
    }
    (pending, in_progress, completed)
}

fn revision_for(snapshot: &PlanSnapshot) -> String {
    crate::stable_text_hash(&serde_json::to_string(snapshot).expect("plan revision json"))
}

#[cfg(test)]
mod tests {
    use super::{
        PlanFocusAction, PlanFocusInput, PlanFocusSnapshot, PlanFocusStatus, PlanItem,
        PlanSnapshot, PlanState, PlanStatus, UpdatePlanInput, UpdatePlanTool,
    };
    use crate::{Tool, ToolExecutionContext};
    use serde_json::json;
    use types::{AgentId, ToolCallId};

    fn sample_items() -> Vec<PlanItem> {
        vec![
            PlanItem {
                step: "Inspect repository".to_string(),
                status: PlanStatus::Completed,
            },
            PlanItem {
                step: "Implement runtime queue".to_string(),
                status: PlanStatus::InProgress,
            },
        ]
    }

    #[tokio::test]
    async fn plan_state_replace_and_snapshot_work() {
        let state = PlanState::new(sample_items());
        assert_eq!(state.snapshot().await.items.len(), 2);
        state
            .replace(PlanSnapshot {
                items: Vec::new(),
                focus: None,
            })
            .await;
        assert!(state.snapshot().await.items.is_empty());
    }

    #[tokio::test]
    async fn update_plan_replaces_shared_plan_snapshot() {
        let state = PlanState::new(sample_items());
        let tool = UpdatePlanTool::new(state.clone());
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(UpdatePlanInput {
                    explanation: Some("Switch to implementation".to_string()),
                    plan: Some(vec![
                        PlanItem {
                            step: "Refine protocol".to_string(),
                            status: PlanStatus::Completed,
                        },
                        PlanItem {
                            step: "Wire host surfaces".to_string(),
                            status: PlanStatus::InProgress,
                        },
                    ]),
                    focus: None,
                    expected_revision: None,
                })
                .unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["kind"], "success");
        assert_eq!(structured["count"], 2);
        assert_eq!(structured["items"][1]["step"], "Wire host surfaces");
        assert_eq!(structured["plan_updated"], true);
        let snapshot = state.snapshot().await;
        assert_eq!(snapshot.items.len(), 2);
        assert_eq!(snapshot.items[1].status, PlanStatus::InProgress);
        assert!(snapshot.focus.is_none());
    }

    #[tokio::test]
    async fn update_plan_focus_can_be_updated_without_replacing_plan() {
        let state = PlanState::new(sample_items());
        let tool = UpdatePlanTool::new(state.clone());
        let context = ToolExecutionContext {
            agent_id: Some(AgentId::from("agent_7")),
            ..Default::default()
        };
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(UpdatePlanInput {
                    explanation: None,
                    plan: None,
                    focus: Some(PlanFocusInput {
                        action: PlanFocusAction::Set,
                        status: Some(PlanFocusStatus::Verifying),
                        summary: Some("Run focused regression".to_string()),
                        next_action: Some("Inspect failures".to_string()),
                        verification: Some("cargo test -p code-agent".to_string()),
                        blocker: None,
                    }),
                    expected_revision: None,
                })
                .unwrap(),
                &context,
            )
            .await
            .unwrap();

        let structured = result.structured_content.unwrap();
        assert_eq!(structured["kind"], "success");
        assert_eq!(structured["plan_updated"], false);
        assert_eq!(structured["focus_updated"], true);
        assert_eq!(structured["focus"]["scope_label"], "agent agent_7");

        let snapshot = state.snapshot().await;
        assert_eq!(snapshot.items, sample_items());
        assert_eq!(
            snapshot.focus,
            Some(PlanFocusSnapshot {
                scope_label: "agent agent_7".to_string(),
                status: PlanFocusStatus::Verifying,
                summary: "Run focused regression".to_string(),
                next_action: Some("Inspect failures".to_string()),
                verification: Some("cargo test -p code-agent".to_string()),
                blocker: None,
            })
        );
    }

    #[tokio::test]
    async fn update_plan_focus_can_be_cleared() {
        let state = PlanState::new_with_snapshot(PlanSnapshot {
            items: sample_items(),
            focus: Some(PlanFocusSnapshot {
                scope_label: "workspace".to_string(),
                status: PlanFocusStatus::Active,
                summary: "Patch observer".to_string(),
                next_action: None,
                verification: None,
                blocker: None,
            }),
        });
        let tool = UpdatePlanTool::new(state.clone());
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "focus": {
                        "action": "clear"
                    }
                }),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        let structured = result.structured_content.unwrap();
        assert_eq!(structured["focus_updated"], true);
        assert!(structured["focus"].is_null());
        assert!(state.snapshot().await.focus.is_none());
    }

    #[tokio::test]
    async fn update_plan_normalizes_multiple_in_progress_steps() {
        let tool = UpdatePlanTool::new(PlanState::default());
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "plan": [
                        {"step": "Inspect", "status": "in_progress"},
                        {"step": "Implement", "status": "in_progress"}
                    ]
                }),
                &ToolExecutionContext::default(),
            )
            .await
            .expect("multiple in_progress steps should normalize");
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["kind"], "success");
        assert_eq!(
            structured["warnings"][0],
            "demoted 1 extra in_progress step(s) to pending so the plan keeps a single active item"
        );
        assert_eq!(structured["items"][0]["status"], "in_progress");
        assert_eq!(structured["items"][1]["status"], "pending");
    }

    #[tokio::test]
    async fn update_plan_respects_revision_guards() {
        let state = PlanState::new(sample_items());
        let tool = UpdatePlanTool::new(state);
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "expected_revision": "stale",
                    "plan": [
                        {"step": "Inspect", "status": "completed"}
                    ]
                }),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["kind"], "error");
        assert_eq!(structured["expected_revision"], "stale");
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn update_plan_requires_a_payload_to_change() {
        let tool = UpdatePlanTool::new(PlanState::default());
        let error = tool
            .execute(
                ToolCallId::new(),
                json!({}),
                &ToolExecutionContext::default(),
            )
            .await
            .expect_err("empty update should fail");
        assert!(error.to_string().contains("requires `plan` and/or `focus`"));
    }

    #[test]
    fn update_plan_spec_is_approval_free_for_internal_coordination() {
        let spec = UpdatePlanTool::new(PlanState::default()).spec();
        assert!(!spec.approval.mutates_state);
        assert!(!spec.approval.open_world);
        assert_eq!(spec.approval.idempotent, Some(true));
    }
}
