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
    AgentId, AgentSessionId, BrowserId, BrowserSummaryRecord, BrowserViewportRecord, SessionId,
    ToolAvailability, ToolCallId, ToolOutputMode, ToolResult, ToolSpec, TurnId,
};

const BROWSER_OPEN_TOOL_NAME: &str = "browser_open";
const BROWSER_SNAPSHOT_TOOL_NAME: &str = "browser_snapshot";
const DEFAULT_BROWSER_TEXT_LINES: usize = 12;
const DEFAULT_BROWSER_ELEMENT_COUNT: usize = 8;
const DEFAULT_BROWSER_HTML_CHARS: usize = 2048;
const MAX_BROWSER_TEXT_LINES: usize = 40;
const MAX_BROWSER_ELEMENT_COUNT: usize = 24;
const MAX_BROWSER_HTML_CHARS: usize = 8192;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserSnapshotRequest {
    pub browser_id: Option<BrowserId>,
    pub include_html: bool,
    pub max_text_lines: usize,
    pub max_elements: usize,
    pub max_html_chars: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserSnapshotElementKind {
    Link,
    Button,
    Input,
    TextArea,
    Select,
    Other,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub struct BrowserSnapshotElement {
    pub kind: BrowserSnapshotElementKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector_hint: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub struct BrowserSnapshotRecord {
    pub browser_id: BrowserId,
    pub current_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text_preview: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interactive_elements: Vec<BrowserSnapshotElement>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub html_preview: Vec<String>,
}

#[async_trait]
pub trait BrowserManager: Send + Sync {
    async fn open_browser(
        &self,
        runtime: BrowserRuntimeContext,
        request: BrowserOpenRequest,
    ) -> Result<BrowserSummaryRecord>;

    async fn snapshot_browser(
        &self,
        runtime: BrowserRuntimeContext,
        request: BrowserSnapshotRequest,
    ) -> Result<BrowserSnapshotRecord>;
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct BrowserOpenToolInput {
    pub url: String,
    #[serde(default)]
    pub headless: Option<bool>,
    #[serde(default)]
    pub viewport: Option<BrowserViewportInput>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct BrowserSnapshotToolInput {
    #[serde(default)]
    pub browser_id: Option<BrowserId>,
    #[serde(default)]
    pub include_html: Option<bool>,
    #[serde(default)]
    pub max_text_lines: Option<usize>,
    #[serde(default)]
    pub max_elements: Option<usize>,
    #[serde(default)]
    pub max_html_chars: Option<usize>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct BrowserOpenToolOutput {
    browser: BrowserSummaryRecord,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct BrowserSnapshotToolOutput {
    snapshot: BrowserSnapshotRecord,
}

#[derive(Clone)]
pub struct BrowserOpenTool {
    manager: Arc<dyn BrowserManager>,
}

#[derive(Clone)]
pub struct BrowserSnapshotTool {
    manager: Arc<dyn BrowserManager>,
}

impl BrowserOpenTool {
    #[must_use]
    pub fn new(manager: Arc<dyn BrowserManager>) -> Self {
        Self { manager }
    }
}

impl BrowserSnapshotTool {
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

#[async_trait]
impl Tool for BrowserSnapshotTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            BROWSER_SNAPSHOT_TOOL_NAME,
            "Inspect an open browser session and return typed page text, interactive element summaries, and an optional bounded HTML preview.",
            serde_json::to_value(schema_for!(BrowserSnapshotToolInput))
                .expect("browser_snapshot schema"),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, false, true).with_network(true),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(BrowserSnapshotToolOutput))
                .expect("browser_snapshot output schema"),
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
        let input: BrowserSnapshotToolInput = serde_json::from_value(arguments)?;
        let request = BrowserSnapshotRequest {
            browser_id: input.browser_id,
            include_html: input.include_html.unwrap_or(false),
            max_text_lines: clamp_snapshot_limit(
                input.max_text_lines,
                DEFAULT_BROWSER_TEXT_LINES,
                MAX_BROWSER_TEXT_LINES,
            ),
            max_elements: clamp_snapshot_limit(
                input.max_elements,
                DEFAULT_BROWSER_ELEMENT_COUNT,
                MAX_BROWSER_ELEMENT_COUNT,
            ),
            max_html_chars: clamp_snapshot_limit(
                input.max_html_chars,
                DEFAULT_BROWSER_HTML_CHARS,
                MAX_BROWSER_HTML_CHARS,
            ),
        };
        let snapshot = self
            .manager
            .snapshot_browser(BrowserRuntimeContext::from(ctx), request)
            .await?;
        Ok(ToolResult::text(
            call_id,
            BROWSER_SNAPSHOT_TOOL_NAME,
            render_browser_snapshot(&snapshot),
        )
        .with_structured_content(json!(BrowserSnapshotToolOutput { snapshot }))
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

fn render_browser_snapshot(snapshot: &BrowserSnapshotRecord) -> String {
    let mut lines = vec![format!("snapshot {}", snapshot.browser_id)];
    lines.push(format!("url {}", snapshot.current_url));
    if let Some(title) = snapshot
        .title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        lines.push(format!("title {title}"));
    }
    lines.push(format!("text {}", snapshot.text_preview.len()));
    lines.push(format!("elements {}", snapshot.interactive_elements.len()));
    if !snapshot.html_preview.is_empty() {
        lines.push(format!("html {}", snapshot.html_preview.len()));
    }
    lines.join("\n")
}

fn clamp_snapshot_limit(candidate: Option<usize>, default: usize, max: usize) -> usize {
    candidate.unwrap_or(default).clamp(1, max)
}

#[cfg(test)]
mod tests {
    use super::{BrowserOpenTool, BrowserSnapshotTool};
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

    #[test]
    fn browser_snapshot_spec_exposes_feature_and_model_constraints() {
        let spec = BrowserSnapshotTool::new(std::sync::Arc::new(FailingManager)).spec();
        assert_eq!(spec.name.as_str(), "browser_snapshot");
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

        async fn snapshot_browser(
            &self,
            _runtime: super::BrowserRuntimeContext,
            _request: super::BrowserSnapshotRequest,
        ) -> crate::Result<super::BrowserSnapshotRecord> {
            unreachable!("spec test does not execute the tool")
        }
    }

    use types::BrowserSummaryRecord;
}
