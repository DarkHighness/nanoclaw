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
const BROWSER_CLICK_TOOL_NAME: &str = "browser_click";
const BROWSER_TYPE_TOOL_NAME: &str = "browser_type";
const BROWSER_EVAL_TOOL_NAME: &str = "browser_eval";
const BROWSER_CLOSE_TOOL_NAME: &str = "browser_close";
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserClickRequest {
    pub browser_id: Option<BrowserId>,
    pub selector: String,
    pub wait_for_navigation: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserTypeRequest {
    pub browser_id: Option<BrowserId>,
    pub selector: String,
    pub text: String,
    pub clear_first: bool,
    pub submit: bool,
    pub wait_for_navigation: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserEvalRequest {
    pub browser_id: Option<BrowserId>,
    pub script: String,
    pub await_promise: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserCloseRequest {
    pub browser_id: Option<BrowserId>,
    pub fire_unload: bool,
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

    async fn click_browser(
        &self,
        runtime: BrowserRuntimeContext,
        request: BrowserClickRequest,
    ) -> Result<BrowserSummaryRecord>;

    async fn type_browser(
        &self,
        runtime: BrowserRuntimeContext,
        request: BrowserTypeRequest,
    ) -> Result<BrowserSummaryRecord>;

    async fn eval_browser(
        &self,
        runtime: BrowserRuntimeContext,
        request: BrowserEvalRequest,
    ) -> Result<(BrowserSummaryRecord, Value)>;

    async fn close_browser(
        &self,
        runtime: BrowserRuntimeContext,
        request: BrowserCloseRequest,
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

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct BrowserClickToolInput {
    #[serde(default)]
    pub browser_id: Option<BrowserId>,
    pub selector: String,
    #[serde(default)]
    pub wait_for_navigation: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct BrowserTypeToolInput {
    #[serde(default)]
    pub browser_id: Option<BrowserId>,
    pub selector: String,
    pub text: String,
    #[serde(default)]
    pub clear_first: Option<bool>,
    #[serde(default)]
    pub submit: Option<bool>,
    #[serde(default)]
    pub wait_for_navigation: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct BrowserEvalToolInput {
    #[serde(default)]
    pub browser_id: Option<BrowserId>,
    pub script: String,
    #[serde(default)]
    pub await_promise: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct BrowserCloseToolInput {
    #[serde(default)]
    pub browser_id: Option<BrowserId>,
    #[serde(default)]
    pub fire_unload: Option<bool>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct BrowserOpenToolOutput {
    browser: BrowserSummaryRecord,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct BrowserSnapshotToolOutput {
    snapshot: BrowserSnapshotRecord,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct BrowserClickToolOutput {
    browser: BrowserSummaryRecord,
    selector: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct BrowserTypeToolOutput {
    browser: BrowserSummaryRecord,
    selector: String,
    text: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct BrowserEvalToolOutput {
    browser: BrowserSummaryRecord,
    result: Value,
    await_promise: bool,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct BrowserCloseToolOutput {
    browser: BrowserSummaryRecord,
    fire_unload: bool,
}

#[derive(Clone)]
pub struct BrowserOpenTool {
    manager: Arc<dyn BrowserManager>,
}

#[derive(Clone)]
pub struct BrowserSnapshotTool {
    manager: Arc<dyn BrowserManager>,
}

#[derive(Clone)]
pub struct BrowserClickTool {
    manager: Arc<dyn BrowserManager>,
}

#[derive(Clone)]
pub struct BrowserTypeTool {
    manager: Arc<dyn BrowserManager>,
}

#[derive(Clone)]
pub struct BrowserEvalTool {
    manager: Arc<dyn BrowserManager>,
}

#[derive(Clone)]
pub struct BrowserCloseTool {
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

impl BrowserClickTool {
    #[must_use]
    pub fn new(manager: Arc<dyn BrowserManager>) -> Self {
        Self { manager }
    }
}

impl BrowserTypeTool {
    #[must_use]
    pub fn new(manager: Arc<dyn BrowserManager>) -> Self {
        Self { manager }
    }
}

impl BrowserEvalTool {
    #[must_use]
    pub fn new(manager: Arc<dyn BrowserManager>) -> Self {
        Self { manager }
    }
}

impl BrowserCloseTool {
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

#[async_trait]
impl Tool for BrowserClickTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            BROWSER_CLICK_TOOL_NAME,
            "Click a DOM element inside an open browser session and persist the resulting browser summary as a typed runtime object update.",
            serde_json::to_value(schema_for!(BrowserClickToolInput)).expect("browser_click schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, true, false, true).with_network(true),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(BrowserClickToolOutput))
                .expect("browser_click output schema"),
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
        let input: BrowserClickToolInput = serde_json::from_value(arguments)?;
        let selector = input.selector.trim();
        if selector.is_empty() {
            return Err(ToolError::invalid(
                "browser_click requires a non-empty selector",
            ));
        }
        let request = BrowserClickRequest {
            browser_id: input.browser_id,
            selector: selector.to_string(),
            wait_for_navigation: input.wait_for_navigation.unwrap_or(false),
        };
        let browser = self
            .manager
            .click_browser(BrowserRuntimeContext::from(ctx), request.clone())
            .await?;
        Ok(ToolResult::text(
            call_id,
            BROWSER_CLICK_TOOL_NAME,
            render_browser_click(&browser, &request.selector),
        )
        .with_structured_content(json!(BrowserClickToolOutput {
            browser,
            selector: request.selector,
        }))
        .with_call_id(external_call_id))
    }
}

#[async_trait]
impl Tool for BrowserTypeTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            BROWSER_TYPE_TOOL_NAME,
            "Type into a DOM element inside an open browser session with explicit clear, submit, and navigation controls, then persist the resulting browser summary as a typed runtime object update.",
            serde_json::to_value(schema_for!(BrowserTypeToolInput)).expect("browser_type schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, true, false, true).with_network(true),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(BrowserTypeToolOutput))
                .expect("browser_type output schema"),
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
        let input: BrowserTypeToolInput = serde_json::from_value(arguments)?;
        let selector = input.selector.trim();
        if selector.is_empty() {
            return Err(ToolError::invalid(
                "browser_type requires a non-empty selector",
            ));
        }
        if input.text.is_empty() {
            return Err(ToolError::invalid("browser_type requires non-empty text"));
        }
        let request = BrowserTypeRequest {
            browser_id: input.browser_id,
            selector: selector.to_string(),
            text: input.text,
            clear_first: input.clear_first.unwrap_or(false),
            submit: input.submit.unwrap_or(false),
            wait_for_navigation: input.wait_for_navigation.unwrap_or(false),
        };
        let browser = self
            .manager
            .type_browser(BrowserRuntimeContext::from(ctx), request.clone())
            .await?;
        Ok(ToolResult::text(
            call_id,
            BROWSER_TYPE_TOOL_NAME,
            render_browser_type(&browser, &request),
        )
        .with_structured_content(json!(BrowserTypeToolOutput {
            browser,
            selector: request.selector,
            text: request.text,
        }))
        .with_call_id(external_call_id))
    }
}

#[async_trait]
impl Tool for BrowserEvalTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            BROWSER_EVAL_TOOL_NAME,
            "Evaluate JavaScript inside an open browser session with explicit promise handling, then persist the resulting browser summary as a typed runtime object update.",
            serde_json::to_value(schema_for!(BrowserEvalToolInput)).expect("browser_eval schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, true, false, true).with_network(true),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(BrowserEvalToolOutput))
                .expect("browser_eval output schema"),
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
        let input: BrowserEvalToolInput = serde_json::from_value(arguments)?;
        let script = input.script.trim();
        if script.is_empty() {
            return Err(ToolError::invalid(
                "browser_eval requires a non-empty script",
            ));
        }
        let request = BrowserEvalRequest {
            browser_id: input.browser_id,
            script: script.to_string(),
            await_promise: input.await_promise.unwrap_or(false),
        };
        let (browser, result) = self
            .manager
            .eval_browser(BrowserRuntimeContext::from(ctx), request.clone())
            .await?;
        Ok(ToolResult::text(
            call_id,
            BROWSER_EVAL_TOOL_NAME,
            render_browser_eval(&browser, &result),
        )
        .with_structured_content(json!(BrowserEvalToolOutput {
            browser,
            result,
            await_promise: request.await_promise,
        }))
        .with_call_id(external_call_id))
    }
}

#[async_trait]
impl Tool for BrowserCloseTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            BROWSER_CLOSE_TOOL_NAME,
            "Close an open browser session and persist the typed browser summary as closed so later browser tools stop targeting the stale session.",
            serde_json::to_value(schema_for!(BrowserCloseToolInput)).expect("browser_close schema"),
            ToolOutputMode::Text,
            tool_approval_profile(false, true, false, true).with_network(true),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(BrowserCloseToolOutput))
                .expect("browser_close output schema"),
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
        let input: BrowserCloseToolInput = serde_json::from_value(arguments)?;
        let request = BrowserCloseRequest {
            browser_id: input.browser_id,
            fire_unload: input.fire_unload.unwrap_or(false),
        };
        let browser = self
            .manager
            .close_browser(BrowserRuntimeContext::from(ctx), request.clone())
            .await?;
        Ok(ToolResult::text(
            call_id,
            BROWSER_CLOSE_TOOL_NAME,
            render_browser_close(&browser),
        )
        .with_structured_content(json!(BrowserCloseToolOutput {
            browser,
            fire_unload: request.fire_unload,
        }))
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

fn render_browser_click(browser: &BrowserSummaryRecord, selector: &str) -> String {
    let mut lines = vec![format!("clicked browser {}", browser.browser_id)];
    lines.push(format!("selector {selector}"));
    lines.push(format!("url {}", browser.current_url));
    lines.push(format!("status {}", browser.status));
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

fn render_browser_type(browser: &BrowserSummaryRecord, request: &BrowserTypeRequest) -> String {
    let mut lines = vec![format!("typed browser {}", browser.browser_id)];
    lines.push(format!("selector {}", request.selector));
    lines.push(format!("text {}", request.text.chars().count()));
    if request.clear_first {
        lines.push("mode replace".to_string());
    }
    if request.submit {
        lines.push("submit enter".to_string());
    }
    lines.push(format!("url {}", browser.current_url));
    lines.push(format!("status {}", browser.status));
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

fn render_browser_eval(browser: &BrowserSummaryRecord, result: &Value) -> String {
    let mut lines = vec![format!("evaluated browser {}", browser.browser_id)];
    lines.push(format!("url {}", browser.current_url));
    lines.push(format!("status {}", browser.status));
    lines.push(format!(
        "result {}",
        match result {
            Value::Null => "null".to_string(),
            Value::Bool(value) => value.to_string(),
            Value::Number(value) => value.to_string(),
            Value::String(value) => value.clone(),
            Value::Array(values) => format!("array {}", values.len()),
            Value::Object(values) => format!("object {}", values.len()),
        }
    ));
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

fn render_browser_close(browser: &BrowserSummaryRecord) -> String {
    let mut lines = vec![format!("closed browser {}", browser.browser_id)];
    lines.push(format!("url {}", browser.current_url));
    lines.push(format!("status {}", browser.status));
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

fn clamp_snapshot_limit(candidate: Option<usize>, default: usize, max: usize) -> usize {
    candidate.unwrap_or(default).clamp(1, max)
}

#[cfg(test)]
mod tests {
    use super::{
        BrowserClickTool, BrowserCloseTool, BrowserEvalTool, BrowserOpenTool, BrowserSnapshotTool,
        BrowserTypeTool,
    };
    use crate::registry::Tool;
    use serde_json::Value;

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

    #[test]
    fn browser_click_spec_exposes_feature_and_model_constraints() {
        let spec = BrowserClickTool::new(std::sync::Arc::new(FailingManager)).spec();
        assert_eq!(spec.name.as_str(), "browser_click");
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
    fn browser_type_spec_exposes_feature_and_model_constraints() {
        let spec = BrowserTypeTool::new(std::sync::Arc::new(FailingManager)).spec();
        assert_eq!(spec.name.as_str(), "browser_type");
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
    fn browser_eval_spec_exposes_feature_and_model_constraints() {
        let spec = BrowserEvalTool::new(std::sync::Arc::new(FailingManager)).spec();
        assert_eq!(spec.name.as_str(), "browser_eval");
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
    fn browser_close_spec_exposes_feature_and_model_constraints() {
        let spec = BrowserCloseTool::new(std::sync::Arc::new(FailingManager)).spec();
        assert_eq!(spec.name.as_str(), "browser_close");
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

        async fn click_browser(
            &self,
            _runtime: super::BrowserRuntimeContext,
            _request: super::BrowserClickRequest,
        ) -> crate::Result<BrowserSummaryRecord> {
            unreachable!("spec test does not execute the tool")
        }

        async fn type_browser(
            &self,
            _runtime: super::BrowserRuntimeContext,
            _request: super::BrowserTypeRequest,
        ) -> crate::Result<BrowserSummaryRecord> {
            unreachable!("spec test does not execute the tool")
        }

        async fn eval_browser(
            &self,
            _runtime: super::BrowserRuntimeContext,
            _request: super::BrowserEvalRequest,
        ) -> crate::Result<(BrowserSummaryRecord, Value)> {
            unreachable!("spec test does not execute the tool")
        }

        async fn close_browser(
            &self,
            _runtime: super::BrowserRuntimeContext,
            _request: super::BrowserCloseRequest,
        ) -> crate::Result<BrowserSummaryRecord> {
            unreachable!("spec test does not execute the tool")
        }
    }

    use types::BrowserSummaryRecord;
}
