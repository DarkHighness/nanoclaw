use super::task::{SubagentParentContext, normalize_optional_non_empty, normalize_paths};
use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::registry::Tool;
use crate::{Result, ToolError, ToolExecutionContext};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use types::{
    CallId, CronScheduleRecord, CronSummaryRecord, ToolCallId, ToolName, ToolOutputMode,
    ToolResult, ToolSpec, new_opaque_id,
};

const CRON_CREATE_TOOL_NAME: &str = "cron_create";

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct CronCreateToolInput {
    pub schedule: CronScheduleInput,
    pub prompt: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub steer: Option<String>,
    #[serde(default)]
    pub allowed_tools: Vec<ToolName>,
    #[serde(default)]
    pub requested_write_set: Vec<String>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub task_id_prefix: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CronScheduleInput {
    OnceAfter {
        delay_seconds: u64,
    },
    EverySeconds {
        interval_seconds: u64,
        #[serde(default)]
        start_after_seconds: Option<u64>,
        #[serde(default)]
        max_runs: Option<u32>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CronTaskTemplate {
    pub role: String,
    pub prompt: String,
    pub steer: Option<String>,
    pub allowed_tools: Vec<ToolName>,
    pub requested_write_set: Vec<String>,
    pub timeout_seconds: Option<u64>,
    pub summary: String,
    pub task_id_prefix: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CronCreateRequest {
    pub schedule: CronScheduleInput,
    pub task_template: CronTaskTemplate,
}

#[async_trait]
pub trait CronManager: Send + Sync {
    async fn create_schedule(
        &self,
        parent: SubagentParentContext,
        request: CronCreateRequest,
    ) -> Result<CronSummaryRecord>;
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct CronCreateToolOutput {
    cron: CronSummaryRecord,
}

#[derive(Clone)]
pub struct CronCreateTool {
    manager: Arc<dyn CronManager>,
}

impl CronCreateTool {
    #[must_use]
    pub fn new(manager: Arc<dyn CronManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl Tool for CronCreateTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            CRON_CREATE_TOOL_NAME,
            "Create a deferred or recurring automation that materializes background task records in the current session. Use this for repeated follow-up work without keeping an interactive turn open.",
            serde_json::to_value(schema_for!(CronCreateToolInput)).expect("cron_create schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, true, false, true),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(CronCreateToolOutput))
                .expect("cron_create output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = CallId::from(&call_id);
        let input: CronCreateToolInput = serde_json::from_value(arguments)?;
        validate_cron_schedule(&input.schedule)?;
        let request = CronCreateRequest {
            schedule: input.schedule.clone(),
            task_template: normalize_cron_task_template(input)?,
        };
        let summary = self
            .manager
            .create_schedule(SubagentParentContext::from(ctx), request)
            .await?;
        Ok(ToolResult::text(
            call_id,
            CRON_CREATE_TOOL_NAME,
            render_cron_create_text(&summary),
        )
        .with_structured_content(json!(CronCreateToolOutput {
            cron: summary.clone(),
        }))
        .with_call_id(external_call_id))
    }
}

fn validate_cron_schedule(schedule: &CronScheduleInput) -> Result<()> {
    match schedule {
        CronScheduleInput::OnceAfter { .. } => Ok(()),
        CronScheduleInput::EverySeconds {
            interval_seconds,
            max_runs,
            ..
        } => {
            if *interval_seconds == 0 {
                return Err(ToolError::invalid(
                    "cron_create recurring schedules require interval_seconds > 0",
                ));
            }
            if max_runs.is_some_and(|max_runs| max_runs == 0) {
                return Err(ToolError::invalid(
                    "cron_create recurring schedules require max_runs > 0 when provided",
                ));
            }
            Ok(())
        }
    }
}

fn normalize_cron_task_template(input: CronCreateToolInput) -> Result<CronTaskTemplate> {
    let prompt = input.prompt.trim().to_string();
    if prompt.is_empty() {
        return Err(ToolError::invalid(
            "cron_create requires a non-empty automation prompt",
        ));
    }
    Ok(CronTaskTemplate {
        role: normalize_optional_non_empty(input.role).unwrap_or_else(|| "general-purpose".into()),
        prompt: prompt.clone(),
        steer: normalize_optional_non_empty(input.steer),
        allowed_tools: input.allowed_tools,
        requested_write_set: normalize_paths(input.requested_write_set),
        timeout_seconds: input.timeout_seconds,
        summary: normalize_optional_non_empty(input.summary)
            .unwrap_or_else(|| summarize_prompt(&prompt)),
        task_id_prefix: normalize_optional_non_empty(input.task_id_prefix)
            .map(|prefix| sanitize_task_id_prefix(&prefix)),
    })
}

fn summarize_prompt(prompt: &str) -> String {
    let first_line = prompt.lines().next().unwrap_or(prompt).trim();
    if first_line.chars().count() > 96 {
        format!("{}...", first_line.chars().take(93).collect::<String>())
    } else {
        first_line.to_string()
    }
}

fn sanitize_task_id_prefix(prefix: &str) -> String {
    let sanitized = prefix
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => ch,
            _ => '_',
        })
        .collect::<String>();
    if sanitized.trim_matches('_').is_empty() {
        format!("task_{}", new_opaque_id())
    } else {
        sanitized
    }
}

fn render_cron_create_text(summary: &CronSummaryRecord) -> String {
    let mut lines = vec![format!("Created automation {}", summary.cron_id)];
    lines.push(format!("role {}", summary.role));
    lines.push(format!("summary {}", summary.prompt_summary));
    lines.push(match &summary.schedule {
        CronScheduleRecord::Once { run_at_unix_s } => format!("once at {run_at_unix_s}"),
        CronScheduleRecord::Recurring {
            interval_seconds,
            next_run_unix_s,
            max_runs,
        } => {
            let mut line = format!("every {interval_seconds}s, next at {next_run_unix_s}");
            if let Some(max_runs) = max_runs {
                line.push_str(&format!(", max {max_runs} run(s)"));
            }
            line
        }
    });
    lines.push(format!("status {}", summary.status));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{
        CronCreateRequest, CronCreateTool, CronCreateToolInput, CronManager, CronScheduleInput,
        CronScheduleRecord, CronSummaryRecord,
    };
    use crate::Result;
    use crate::{Tool, ToolExecutionContext};
    use async_trait::async_trait;
    use nanoclaw_test_support::run_current_thread_test;
    use serde_json::Value;
    use std::sync::{Arc, Mutex};
    use types::{AgentSessionId, CronId, CronStatus, MessagePart, SessionId, ToolCallId};

    macro_rules! bounded_async_test {
        (async fn $name:ident() $body:block) => {
            #[test]
            fn $name() {
                run_current_thread_test(async $body);
            }
        };
    }

    #[derive(Default)]
    struct FakeCronManager {
        last_request: Mutex<Option<CronCreateRequest>>,
    }

    #[async_trait]
    impl CronManager for FakeCronManager {
        async fn create_schedule(
            &self,
            parent: super::SubagentParentContext,
            request: CronCreateRequest,
        ) -> Result<CronSummaryRecord> {
            *self.last_request.lock().unwrap() = Some(request.clone());
            Ok(CronSummaryRecord {
                cron_id: CronId::from("cron_123"),
                session_id: parent
                    .session_id
                    .unwrap_or_else(|| SessionId::from("session_1")),
                agent_session_id: parent
                    .agent_session_id
                    .unwrap_or_else(|| AgentSessionId::from("agent_session_1")),
                parent_agent_id: parent.parent_agent_id,
                latest_task_id: None,
                role: request.task_template.role,
                prompt_summary: request.task_template.summary,
                status: CronStatus::Scheduled,
                schedule: CronScheduleRecord::Once { run_at_unix_s: 42 },
                created_at_unix_s: 7,
                last_run_at_unix_s: None,
                run_count: 0,
            })
        }
    }

    fn context() -> ToolExecutionContext {
        ToolExecutionContext {
            session_id: Some(SessionId::from("session_1")),
            agent_session_id: Some(AgentSessionId::from("agent_session_1")),
            ..Default::default()
        }
    }

    bounded_async_test!(
        async fn cron_create_normalizes_task_template_and_returns_typed_summary() {
            let manager = Arc::new(FakeCronManager::default());
            let result = CronCreateTool::new(manager.clone())
                .execute(
                    ToolCallId::new(),
                    serde_json::to_value(CronCreateToolInput {
                        schedule: CronScheduleInput::EverySeconds {
                            interval_seconds: 300,
                            start_after_seconds: Some(0),
                            max_runs: Some(3),
                        },
                        prompt: "Review the changelog and open a follow-up task.".to_string(),
                        role: Some("reviewer".to_string()),
                        steer: Some("focus on regressions".to_string()),
                        allowed_tools: Vec::new(),
                        requested_write_set: vec![" src/lib.rs ".to_string()],
                        timeout_seconds: Some(90),
                        summary: None,
                        task_id_prefix: Some("nightly-review".to_string()),
                    })
                    .unwrap(),
                    &context(),
                )
                .await
                .unwrap();

            assert!(!result.is_error);
            let structured = result.structured_content.unwrap();
            let cron = structured.get("cron").expect("cron output");
            assert_eq!(
                cron.get("cron_id").and_then(Value::as_str),
                Some("cron_123")
            );
            assert_eq!(
                cron.get("status").and_then(Value::as_str),
                Some("scheduled")
            );
            let MessagePart::Text { text } = &result.parts[0] else {
                panic!("expected text output");
            };
            assert!(text.contains("Created automation cron_123"));

            let request = manager
                .last_request
                .lock()
                .unwrap()
                .clone()
                .expect("request captured");
            assert_eq!(request.task_template.role, "reviewer");
            assert_eq!(
                request.task_template.summary,
                "Review the changelog and open a follow-up task."
            );
            assert_eq!(
                request.task_template.requested_write_set,
                vec!["src/lib.rs"]
            );
            assert_eq!(
                request.task_template.task_id_prefix.as_deref(),
                Some("nightly-review")
            );
        }
    );

    bounded_async_test!(
        async fn cron_create_rejects_zero_interval_recurring_schedules() {
            let manager = Arc::new(FakeCronManager::default());
            let error = CronCreateTool::new(manager)
                .execute(
                    ToolCallId::new(),
                    serde_json::to_value(CronCreateToolInput {
                        schedule: CronScheduleInput::EverySeconds {
                            interval_seconds: 0,
                            start_after_seconds: None,
                            max_runs: None,
                        },
                        prompt: "Ping".to_string(),
                        role: None,
                        steer: None,
                        allowed_tools: Vec::new(),
                        requested_write_set: Vec::new(),
                        timeout_seconds: None,
                        summary: None,
                        task_id_prefix: None,
                    })
                    .unwrap(),
                    &context(),
                )
                .await
                .unwrap_err();

            assert!(error.to_string().contains("interval_seconds > 0"));
        }
    );
}
