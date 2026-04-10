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

#[derive(Clone, Debug, Default)]
pub struct PlanState {
    items: Arc<Mutex<Vec<PlanItem>>>,
}

impl PlanState {
    #[must_use]
    pub fn new(initial_items: Vec<PlanItem>) -> Self {
        Self {
            items: Arc::new(Mutex::new(initial_items)),
        }
    }

    pub async fn snapshot(&self) -> Vec<PlanItem> {
        self.items.lock().expect("plan state lock").clone()
    }

    pub async fn replace(&self, items: Vec<PlanItem>) {
        *self.items.lock().expect("plan state lock") = items;
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct UpdatePlanInput {
    #[serde(default)]
    pub explanation: Option<String>,
    pub plan: Vec<PlanItem>,
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
        revision_before: String,
        revision_after: String,
        pending_count: usize,
        in_progress_count: usize,
        completed_count: usize,
        items: Vec<PlanItem>,
    },
    Error {
        explanation: Option<String>,
        expected_revision: String,
        revision_before: String,
        items: Vec<PlanItem>,
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
            "Updates the task plan. Provide an optional explanation and a list of plan items, each with a step and status. The shared plan keeps at most one step in_progress at a time, and extra in_progress items are demoted to pending.",
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
        _ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: UpdatePlanInput = serde_json::from_value(arguments)?;
        let normalized = normalize_plan_items(input.plan)?;
        let before = self.state.snapshot().await;
        let revision_before = revision_for(&before);

        if let Some(expected_revision) = input.expected_revision.as_deref()
            && expected_revision != revision_before
        {
            let text = format!(
                "Plan revision mismatch. Expected {expected_revision}, found {revision_before}. Re-read the current plan before updating it."
            );
            let structured_output = UpdatePlanToolOutput::Error {
                explanation: input.explanation.clone(),
                expected_revision: expected_revision.to_string(),
                revision_before: revision_before.clone(),
                items: before.clone(),
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
                    "items": before,
                })),
                is_error: true,
            });
        }

        self.state.replace(normalized.items.clone()).await;
        let revision_after = revision_for(&normalized.items);
        let (pending_count, in_progress_count, completed_count) = status_counts(&normalized.items);
        let structured_output = UpdatePlanToolOutput::Success {
            explanation: input.explanation.clone(),
            warnings: normalized.warnings.clone(),
            count: normalized.items.len(),
            revision_before: revision_before.clone(),
            revision_after: revision_after.clone(),
            pending_count,
            in_progress_count,
            completed_count,
            items: normalized.items.clone(),
        };

        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "update_plan".into(),
            parts: vec![MessagePart::text(render_plan_update(
                input.explanation.as_deref(),
                &normalized.items,
                &normalized.warnings,
                &revision_before,
                &revision_after,
            ))],
            attachments: Vec::new(),
            structured_content: Some(
                serde_json::to_value(structured_output).expect("update_plan structured output"),
            ),
            continuation: None,
            metadata: Some(serde_json::json!({
                "count": normalized.items.len(),
                "revision_before": revision_before,
                "revision_after": revision_after,
                "pending_count": pending_count,
                "in_progress_count": in_progress_count,
                "completed_count": completed_count,
                "warnings": normalized.warnings,
                "items": normalized.items,
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

fn render_plan_update(
    explanation: Option<&str>,
    items: &[PlanItem],
    warnings: &[String],
    revision_before: &str,
    revision_after: &str,
) -> String {
    let mut sections = vec![format!(
        "[plan count={} revision {} -> {}]",
        items.len(),
        revision_before,
        revision_after
    )];
    if let Some(explanation) = explanation.map(str::trim).filter(|value| !value.is_empty()) {
        sections.push(format!("explanation> {explanation}"));
    }
    sections.extend(warnings.iter().map(|warning| format!("warning> {warning}")));
    sections.push(String::new());
    sections.push(render_plan(items));
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

fn revision_for(items: &[PlanItem]) -> String {
    crate::stable_text_hash(&serde_json::to_string(items).expect("plan revision json"))
}

#[cfg(test)]
mod tests {
    use super::{PlanItem, PlanState, PlanStatus, UpdatePlanInput, UpdatePlanTool};
    use crate::{Tool, ToolExecutionContext};
    use serde_json::json;
    use types::ToolCallId;

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
        assert_eq!(state.snapshot().await.len(), 2);
        state.replace(Vec::new()).await;
        assert!(state.snapshot().await.is_empty());
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
                    plan: vec![
                        PlanItem {
                            step: "Refine protocol".to_string(),
                            status: PlanStatus::Completed,
                        },
                        PlanItem {
                            step: "Wire host surfaces".to_string(),
                            status: PlanStatus::InProgress,
                        },
                    ],
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
        let snapshot = state.snapshot().await;
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[1].status, PlanStatus::InProgress);
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

    #[test]
    fn update_plan_spec_is_approval_free_for_internal_coordination() {
        let spec = UpdatePlanTool::new(PlanState::default()).spec();
        assert!(!spec.approval.mutates_state);
        assert!(!spec.approval.open_world);
        assert_eq!(spec.approval.idempotent, Some(true));
    }
}
