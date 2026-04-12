use crate::HOST_FEATURE_HOST_PROCESS_SURFACES;
use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::registry::Tool;
use crate::{Result, ToolError, ToolExecutionContext};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use types::{
    AgentId, AgentSessionId, BrowserSummaryRecord, BrowserViewportRecord, SessionId,
    ToolAvailability, ToolCallId, ToolOutputMode, ToolResult, ToolSpec, TurnId,
};

const BROWSER_OPEN_TOOL_NAME: &str = "browser_open";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BrowserRuntimeContext {
    pub session_id: Option<SessionId>,
    pub agent_session_id: Option<AgentSessionId>,
    pub turn_id: Option<TurnId>,
    pub parent_agent_id: Option<AgentId>,
    pub task_id: Option<types::TaskId>,
}

impl From<&ToolExecutionContext> for BrowserRuntimeContext {
    fn from(ctx: &ToolExecutionContext) -> Self {
        Self {
            session_id: ctx.session_id.clone(),
            agent_session_id: ctx.agent_session_id.clone(),
            turn_id: ctx.turn_id.clone(),
            parent_agent_id: ctx.agent_id.clone(),
            task_id: ctx.task_id.as_deref().map(types::TaskId::from),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub struct BrowserViewportInput {
    pub width: u32,
    pub height: u32,
}

impl BrowserViewportInput {
    #[must_use]
    pub fn into_record(self) -> BrowserViewportRecord {
        BrowserViewportRecord {
            width: self.width,
            height: self.height,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserOpenRequest {
    pub url: String,
    pub headless: bool,
    pub viewport: Option<BrowserViewportRecord>,
}

#[async_trait]
pub trait BrowserManager: Send + Sync {
    async fn open_browser(
        &self,
        runtime: BrowserRuntimeContext,
        request: BrowserOpenRequest,
    ) -> Result<BrowserSummaryRecord>;
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct BrowserOpenToolInput {
    pub url: String,
    #[serde(default)]
    pub headless: Option<bool>,
    #[serde(default)]
    pub viewport: Option<BrowserViewportInput>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct BrowserOpenToolOutput {
    browser: BrowserSummaryRecord,
}

#[derive(Clone)]
pub struct BrowserOpenTool {
    manager: Arc<dyn BrowserManager>,
}

impl BrowserOpenTool {
    #[must_use]
    pub fn new(manager: Arc<dyn BrowserManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl Tool for BrowserOpenTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            BROWSER_OPEN_TOOL_NAME,
            "Open a browser session, navigate to a page, and persist the browser session as a typed runtime object for follow-up browser automation tools.",
            serde_json::to_value(schema_for!(BrowserOpenToolInput)).expect("browser_open schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, true, false, true).with_network(true),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(BrowserOpenToolOutput))
                .expect("browser_open output schema"),
        )
        .with_availability(ToolAvailability {
            feature_flags: vec![HOST_FEATURE_HOST_PROCESS_SURFACES.to_string()],
            provider_allowlist: vec!["openai".to_string()],
            model_allowlist: vec!["gpt-5*".to_string()],
            ..ToolAvailability::default()
        })
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: BrowserOpenToolInput = serde_json::from_value(arguments)?;
        let url = input.url.trim();
        if url.is_empty() {
            return Err(ToolError::invalid("browser_open requires a non-empty url"));
        }
        let request = BrowserOpenRequest {
            url: url.to_string(),
            headless: input.headless.unwrap_or(true),
            viewport: input.viewport.map(BrowserViewportInput::into_record),
        };
        let browser = self
            .manager
            .open_browser(BrowserRuntimeContext::from(ctx), request)
            .await?;
        Ok(ToolResult::text(
            call_id,
            BROWSER_OPEN_TOOL_NAME,
            render_browser_summary(&browser),
        )
        .with_structured_content(json!(BrowserOpenToolOutput { browser }))
        .with_call_id(external_call_id))
    }
}

fn render_browser_summary(browser: &BrowserSummaryRecord) -> String {
    let mut lines = vec![format!("opened browser {}", browser.browser_id)];
    lines.push(format!("url {}", browser.current_url));
    lines.push(format!("status {}", browser.status));
    lines.push(if browser.headless {
        "mode headless".to_string()
    } else {
        "mode headful".to_string()
    });
    if let Some(viewport) = browser.viewport.as_ref() {
        lines.push(format!("viewport {}x{}", viewport.width, viewport.height));
    }
    if let Some(title) = browser
        .title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        lines.push(format!("title {title}"));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::BrowserOpenTool;
    use crate::registry::Tool;

    #[test]
    fn browser_open_spec_exposes_feature_and_model_constraints() {
        let spec = BrowserOpenTool::new(std::sync::Arc::new(FailingManager)).spec();
        assert_eq!(spec.name.as_str(), "browser_open");
        assert_eq!(
            spec.availability.feature_flags,
            vec![crate::HOST_FEATURE_HOST_PROCESS_SURFACES.to_string()]
        );
        assert_eq!(
            spec.availability.provider_allowlist,
            vec!["openai".to_string()]
        );
        assert_eq!(
            spec.availability.model_allowlist,
            vec!["gpt-5*".to_string()]
        );
    }

    struct FailingManager;

    #[async_trait::async_trait]
    impl super::BrowserManager for FailingManager {
        async fn open_browser(
            &self,
            _runtime: super::BrowserRuntimeContext,
            _request: super::BrowserOpenRequest,
        ) -> crate::Result<BrowserSummaryRecord> {
            unreachable!("spec test does not execute the tool")
        }
    }

    use types::BrowserSummaryRecord;
}
