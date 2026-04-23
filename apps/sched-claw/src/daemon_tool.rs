use crate::daemon_client::SchedExtDaemonClient;
use crate::daemon_protocol::{SchedExtDaemonRequest, SchedExtDaemonResponse};
use crate::display::{OutputStyle, render_daemon_response};
use agent::tools::Tool;
use agent::types::{ToolApprovalProfile, ToolOrigin, ToolOutputMode, ToolSource, ToolSpec};
use agent::{ToolCallId, ToolExecutionContext, ToolResult};
use async_trait::async_trait;
use schemars::schema_for;
use serde_json::Value;

const SCHED_CLAW_DAEMON_TOOL_NAME: &str = "sched_claw_daemon";

#[derive(Clone, Debug)]
pub struct SchedClawDaemonTool {
    client: SchedExtDaemonClient,
}

impl SchedClawDaemonTool {
    #[must_use]
    pub fn new(client: SchedExtDaemonClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Tool for SchedClawDaemonTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            SCHED_CLAW_DAEMON_TOOL_NAME,
            "Call the privileged sched-claw daemon for bounded rollout control and structured privileged scheduler evidence capture. Discover capability boundaries first, then use only the constrained actions it exposes; do not treat it as a generic root shell.",
            serde_json::to_value(schema_for!(SchedExtDaemonRequest))
                .expect("sched_claw_daemon schema"),
            ToolOutputMode::Text,
            ToolOrigin::Local,
            ToolSource::Dynamic,
        )
        .with_aliases(vec!["daemon".into()])
        .with_output_schema(
            serde_json::to_value(schema_for!(SchedExtDaemonResponse))
                .expect("sched_claw_daemon output schema"),
        )
        .with_approval(
            ToolApprovalProfile::new(false, true, Some(false), false)
                .with_host_escape(true)
                .with_approval_message(
                    "This tool reaches a privileged daemon that can replace the active Linux scheduler and run bounded privileged scheduler capture.",
                ),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        _ctx: &ToolExecutionContext,
    ) -> agent::tools::Result<ToolResult> {
        let request: SchedExtDaemonRequest = serde_json::from_value(arguments)?;
        let response = match self.client.send(&request).await {
            Ok(response) => response,
            Err(error) => {
                return Ok(ToolResult::error(
                    call_id,
                    SCHED_CLAW_DAEMON_TOOL_NAME,
                    format!(
                        "failed to reach daemon socket {}: {error:#}",
                        self.client.socket_path().display()
                    ),
                ));
            }
        };
        let rendered = render_daemon_response(&response, OutputStyle::Plain);
        match &response {
            SchedExtDaemonResponse::Error { .. } => {
                Ok(
                    ToolResult::error(call_id, SCHED_CLAW_DAEMON_TOOL_NAME, rendered)
                        .with_structured_content(serde_json::to_value(response)?),
                )
            }
            _ => Ok(
                ToolResult::text(call_id, SCHED_CLAW_DAEMON_TOOL_NAME, rendered)
                    .with_structured_content(serde_json::to_value(response)?),
            ),
        }
    }
}
