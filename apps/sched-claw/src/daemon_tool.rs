use crate::daemon_client::SchedExtDaemonClient;
use crate::daemon_protocol::{SchedExtDaemonRequest, SchedExtDaemonResponse};
use crate::display::{OutputStyle, render_daemon_response};
use agent::tools::Tool;
use agent::types::{ToolApprovalProfile, ToolOrigin, ToolOutputMode, ToolSource, ToolSpec};
use agent::{ToolCallId, ToolExecutionContext, ToolResult};
use async_trait::async_trait;
use schemars::schema_for;
use serde_json::Value;

const SCHED_EXT_DAEMON_TOOL_NAME: &str = "sched_ext_daemon";

#[derive(Clone, Debug)]
pub struct SchedExtDaemonTool {
    client: SchedExtDaemonClient,
}

impl SchedExtDaemonTool {
    #[must_use]
    pub fn new(client: SchedExtDaemonClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Tool for SchedExtDaemonTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::function(
            SCHED_EXT_DAEMON_TOOL_NAME,
            "Call the privileged sched-ext daemon for constrained scheduler lifecycle work and structured privileged scheduler evidence capture. Use it only for status, activate, stop, logs, collect_perf, and collect_sched; do not treat it as a generic root shell.",
            serde_json::to_value(schema_for!(SchedExtDaemonRequest))
                .expect("sched_ext_daemon schema"),
            ToolOutputMode::Text,
            ToolOrigin::Local,
            ToolSource::Dynamic,
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(SchedExtDaemonResponse))
                .expect("sched_ext_daemon output schema"),
        )
        .with_approval(
            ToolApprovalProfile::new(false, true, Some(false), false)
                .with_host_escape(true)
                .with_approval_message(
                    "This tool reaches a privileged daemon that can replace the active Linux scheduler and run structured privileged perf capture.",
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
                    SCHED_EXT_DAEMON_TOOL_NAME,
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
                    ToolResult::error(call_id, SCHED_EXT_DAEMON_TOOL_NAME, rendered)
                        .with_structured_content(serde_json::to_value(response)?),
                )
            }
            _ => Ok(
                ToolResult::text(call_id, SCHED_EXT_DAEMON_TOOL_NAME, rendered)
                    .with_structured_content(serde_json::to_value(response)?),
            ),
        }
    }
}
