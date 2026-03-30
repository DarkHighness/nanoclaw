use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::registry::Tool;
use crate::{Result, ToolError, ToolExecutionContext};
use async_trait::async_trait;
use dashmap::DashMap;
use rquickjs::context::intrinsic;
use rquickjs::function::{Func, MutFn, Rest};
use rquickjs::{Coerced, Context, Ctx, Exception, FromJs, Object, Runtime, Value as JsValue};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::task::spawn_blocking;
use types::{MessagePart, ToolCallId, ToolOutputMode, ToolResult, ToolSpec, new_opaque_id};

const DEFAULT_TIMEOUT_MS: u64 = 1_000;
const MAX_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_MAX_OUTPUT_CHARS: usize = 8 * 1024;
const MAX_ALLOWED_OUTPUT_CHARS: usize = 64 * 1024;
const MAX_CODE_CHARS: usize = 16 * 1024;
const MAX_SESSION_SNIPPETS: usize = 64;
const MAX_SESSION_SOURCE_CHARS: usize = 128 * 1024;
const MAX_TRACKED_JS_REPL_SESSIONS: usize = 128;
const JS_RUNTIME_MEMORY_LIMIT_BYTES: usize = 16 * 1024 * 1024;
const JS_RUNTIME_STACK_LIMIT_BYTES: usize = 512 * 1024;
const JS_RUNTIME_GC_THRESHOLD_BYTES: usize = 4 * 1024 * 1024;
const RESTRICTED_GLOBALS: &[&str] = &[
    "eval",
    "Function",
    "AsyncFunction",
    "GeneratorFunction",
    "AsyncGeneratorFunction",
    "WebAssembly",
];

type ReplIntrinsics = (
    intrinsic::Date,
    intrinsic::Eval,
    intrinsic::RegExpCompiler,
    intrinsic::RegExp,
    intrinsic::Json,
    intrinsic::MapSet,
    intrinsic::TypedArrays,
    intrinsic::Promise,
    intrinsic::BigInt,
);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct JsReplSessionId(String);

impl JsReplSessionId {
    fn new() -> Self {
        Self(format!("js-{}", new_opaque_id()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for JsReplSessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<&str> for JsReplSessionId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JsReplMode {
    Start,
    Eval,
    Reset,
    Close,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct JsReplToolInput {
    #[serde(default)]
    pub mode: Option<JsReplMode>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub max_output_chars: Option<usize>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct JsReplLogOutput {
    level: String,
    text: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct JsReplResultOutput {
    type_name: String,
    preview: String,
    preview_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    json: Option<Value>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum JsReplToolOutput {
    Start {
        session_id: String,
        state: String,
        created_at_unix_s: u64,
        snippet_count: usize,
        total_source_chars: usize,
    },
    Eval {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        persisted: bool,
        timeout_ms: u64,
        max_output_chars: usize,
        output_truncated: bool,
        log_count: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        snippet_count: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total_source_chars: Option<usize>,
        result: JsReplResultOutput,
        logs: Vec<JsReplLogOutput>,
    },
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        persisted: bool,
        timeout_ms: u64,
        max_output_chars: usize,
        timed_out: bool,
        output_truncated: bool,
        log_count: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        snippet_count: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total_source_chars: Option<usize>,
        error: String,
        logs: Vec<JsReplLogOutput>,
    },
    Reset {
        session_id: String,
        state: String,
        cleared_snippet_count: usize,
    },
    Close {
        session_id: String,
        state: String,
        closed: bool,
    },
}

#[derive(Clone)]
pub struct JsReplTool;

impl fmt::Debug for JsReplTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsReplTool").finish_non_exhaustive()
    }
}

impl Default for JsReplTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct JsReplSession {
    id: JsReplSessionId,
    created_at_unix_s: u64,
    last_used_at_unix_s: Mutex<u64>,
    state: Mutex<JsReplSessionState>,
}

#[derive(Debug, Default)]
struct JsReplSessionState {
    snippets: Vec<String>,
    total_source_chars: usize,
}

#[derive(Clone, Debug)]
struct JsEvalOutcome {
    result: JsReplResultOutput,
    logs: Vec<JsReplLogOutput>,
    output_truncated: bool,
}

#[derive(Clone, Debug)]
struct JsEvalFailure {
    message: String,
    logs: Vec<JsReplLogOutput>,
    output_truncated: bool,
    timed_out: bool,
}

#[derive(Debug)]
struct OutputCapture {
    logs: Vec<JsReplLogOutput>,
    captured_chars: usize,
    max_chars: usize,
    truncated: bool,
    suppressed: bool,
}

type SessionRegistry = DashMap<JsReplSessionId, Arc<JsReplSession>>;

static JS_REPL_SESSIONS: OnceLock<SessionRegistry> = OnceLock::new();

impl JsReplTool {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for JsReplTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "js_repl",
            "Evaluate JavaScript in a controlled in-memory REPL. Optional sessions preserve state by replaying prior successful snippets; the runtime does not expose filesystem, shell, or network APIs.",
            serde_json::to_value(schema_for!(JsReplToolInput)).expect("js_repl schema"),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(JsReplToolOutput))
                .expect("js_repl output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        _ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: JsReplToolInput = serde_json::from_value(arguments)?;
        match input.mode.unwrap_or(JsReplMode::Eval) {
            JsReplMode::Start => execute_start(call_id).await,
            JsReplMode::Eval => execute_eval(call_id, input).await,
            JsReplMode::Reset => execute_reset(call_id, input).await,
            JsReplMode::Close => execute_close(call_id, input).await,
        }
    }
}

impl JsReplSession {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            id: JsReplSessionId::new(),
            created_at_unix_s: unix_timestamp_s(),
            last_used_at_unix_s: Mutex::new(unix_timestamp_s()),
            state: Mutex::new(JsReplSessionState::default()),
        })
    }

    fn touch(&self) {
        *self
            .last_used_at_unix_s
            .lock()
            .expect("js_repl last_used_at lock") = unix_timestamp_s();
    }

    fn last_used_at_unix_s(&self) -> u64 {
        *self
            .last_used_at_unix_s
            .lock()
            .expect("js_repl last_used_at lock")
    }

    fn snapshot(&self) -> (Vec<String>, usize, usize) {
        let state = self.state.lock().expect("js_repl session state lock");
        (
            state.snippets.clone(),
            state.snippets.len(),
            state.total_source_chars,
        )
    }

    fn can_accept(&self, code: &str) -> std::result::Result<(), String> {
        let state = self.state.lock().expect("js_repl session state lock");
        let next_snippets = state.snippets.len() + 1;
        let next_chars = state.total_source_chars + code.chars().count();
        if next_snippets > MAX_SESSION_SNIPPETS || next_chars > MAX_SESSION_SOURCE_CHARS {
            return Err(format!(
                "js_repl session `{}` reached its history budget ({} snippets / {} chars). Reset or close the session before adding more code.",
                self.id, MAX_SESSION_SNIPPETS, MAX_SESSION_SOURCE_CHARS
            ));
        }
        Ok(())
    }

    fn append(&self, code: String) -> (usize, usize) {
        let mut state = self.state.lock().expect("js_repl session state lock");
        state.total_source_chars += code.chars().count();
        state.snippets.push(code);
        (state.snippets.len(), state.total_source_chars)
    }

    fn clear(&self) -> usize {
        let mut state = self.state.lock().expect("js_repl session state lock");
        let cleared = state.snippets.len();
        state.snippets.clear();
        state.total_source_chars = 0;
        cleared
    }
}

impl OutputCapture {
    fn new(max_chars: usize) -> Self {
        Self {
            logs: Vec::new(),
            captured_chars: 0,
            max_chars,
            truncated: false,
            suppressed: false,
        }
    }

    fn push(&mut self, level: &str, text: String) {
        if self.suppressed {
            return;
        }
        if self.captured_chars >= self.max_chars {
            self.truncated = true;
            return;
        }
        let remaining = self.max_chars - self.captured_chars;
        let (visible, truncated) = truncate_text_with_limit(&text, remaining);
        if visible.is_empty() {
            self.truncated = true;
            return;
        }
        self.captured_chars += visible.chars().count();
        self.truncated |= truncated;
        self.logs.push(JsReplLogOutput {
            level: level.to_string(),
            text: visible,
        });
    }
}

async fn execute_start(call_id: ToolCallId) -> Result<ToolResult> {
    let external_call_id = types::CallId::from(&call_id);
    let session = JsReplSession::new();
    insert_session(session.clone());

    Ok(ToolResult {
        id: call_id,
        call_id: external_call_id,
        tool_name: "js_repl".into(),
        parts: vec![MessagePart::text(format!(
            "[js_repl mode=start]\nsession_id> {}\nstate> ready\n\nUse the returned session_id with mode=\"eval\" to preserve JavaScript state across calls.",
            session.id
        ))],
        structured_content: Some(
            serde_json::to_value(JsReplToolOutput::Start {
                session_id: session.id.as_str().to_string(),
                state: "ready".to_string(),
                created_at_unix_s: session.created_at_unix_s,
                snippet_count: 0,
                total_source_chars: 0,
            })
            .expect("js_repl start output"),
        ),
        metadata: Some(serde_json::json!({
            "mode": "start",
            "session_id": session.id.as_str(),
            "state": "ready",
            "created_at_unix_s": session.created_at_unix_s,
            "snippet_count": 0,
            "total_source_chars": 0,
        })),
        is_error: false,
    })
}

async fn execute_eval(call_id: ToolCallId, input: JsReplToolInput) -> Result<ToolResult> {
    let external_call_id = types::CallId::from(&call_id);
    let code = resolve_code(&input)?;
    let timeout_ms = input
        .timeout_ms
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(1, MAX_TIMEOUT_MS);
    let max_output_chars = input
        .max_output_chars
        .unwrap_or(DEFAULT_MAX_OUTPUT_CHARS)
        .clamp(1, MAX_ALLOWED_OUTPUT_CHARS);

    let session = input
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(JsReplSessionId::from)
        .and_then(|session_id| get_session(&session_id));
    if input.session_id.is_some() && session.is_none() {
        return Ok(ToolResult::error(
            call_id,
            "js_repl",
            format!(
                "Unknown js_repl session_id `{}`",
                input.session_id.unwrap_or_default()
            ),
        )
        .with_call_id(external_call_id));
    }

    let (history, snippet_count, total_source_chars) = if let Some(session) = &session {
        session.touch();
        if let Err(error) = session.can_accept(&code) {
            return Ok(ToolResult::error(call_id, "js_repl", error).with_call_id(external_call_id));
        }
        session.snapshot()
    } else {
        (Vec::new(), 0, 0)
    };

    let code_for_eval = code.clone();
    let evaluation = spawn_blocking(move || {
        evaluate_javascript(history, code_for_eval, timeout_ms, max_output_chars)
    })
    .await
    .map_err(|error| ToolError::invalid_state(format!("js_repl worker failed: {error}")))?;

    let session_id = session.as_ref().map(|value| value.id.as_str().to_string());
    let persisted = session.is_some();
    match evaluation {
        Ok(outcome) => {
            let (next_snippet_count, next_total_source_chars) = if let Some(session) = &session {
                session.touch();
                session.append(code)
            } else {
                (snippet_count, total_source_chars)
            };

            let text = render_eval_output(
                session_id.as_deref(),
                persisted,
                timeout_ms,
                &outcome.result,
                &outcome.logs,
                outcome.output_truncated,
            );
            Ok(ToolResult {
                id: call_id,
                call_id: external_call_id,
                tool_name: "js_repl".into(),
                parts: vec![MessagePart::text(text)],
                structured_content: Some(
                    serde_json::to_value(JsReplToolOutput::Eval {
                        session_id: session_id.clone(),
                        persisted,
                        timeout_ms,
                        max_output_chars,
                        output_truncated: outcome.output_truncated,
                        log_count: outcome.logs.len(),
                        snippet_count: session.as_ref().map(|_| next_snippet_count),
                        total_source_chars: session.as_ref().map(|_| next_total_source_chars),
                        result: outcome.result.clone(),
                        logs: outcome.logs.clone(),
                    })
                    .expect("js_repl eval output"),
                ),
                metadata: Some(serde_json::json!({
                    "mode": "eval",
                    "session_id": session_id,
                    "persisted": persisted,
                    "timeout_ms": timeout_ms,
                    "max_output_chars": max_output_chars,
                    "output_truncated": outcome.output_truncated,
                    "log_count": outcome.logs.len(),
                    "snippet_count": session.as_ref().map(|_| next_snippet_count),
                    "total_source_chars": session.as_ref().map(|_| next_total_source_chars),
                    "result_type": outcome.result.type_name,
                    "result_preview_truncated": outcome.result.preview_truncated,
                })),
                is_error: false,
            })
        }
        Err(error) => {
            let text = render_error_output(
                session_id.as_deref(),
                persisted,
                timeout_ms,
                &error.message,
                &error.logs,
                error.output_truncated,
                error.timed_out,
            );
            Ok(ToolResult {
                id: call_id,
                call_id: external_call_id,
                tool_name: "js_repl".into(),
                parts: vec![MessagePart::text(text)],
                structured_content: Some(
                    serde_json::to_value(JsReplToolOutput::Error {
                        session_id: session_id.clone(),
                        persisted,
                        timeout_ms,
                        max_output_chars,
                        timed_out: error.timed_out,
                        output_truncated: error.output_truncated,
                        log_count: error.logs.len(),
                        snippet_count: session.as_ref().map(|_| snippet_count),
                        total_source_chars: session.as_ref().map(|_| total_source_chars),
                        error: error.message.clone(),
                        logs: error.logs.clone(),
                    })
                    .expect("js_repl error output"),
                ),
                metadata: Some(serde_json::json!({
                    "mode": "eval",
                    "session_id": session_id,
                    "persisted": persisted,
                    "timeout_ms": timeout_ms,
                    "max_output_chars": max_output_chars,
                    "timed_out": error.timed_out,
                    "output_truncated": error.output_truncated,
                    "log_count": error.logs.len(),
                    "snippet_count": session.as_ref().map(|_| snippet_count),
                    "total_source_chars": session.as_ref().map(|_| total_source_chars),
                    "error": error.message,
                })),
                is_error: true,
            })
        }
    }
}

async fn execute_reset(call_id: ToolCallId, input: JsReplToolInput) -> Result<ToolResult> {
    let external_call_id = types::CallId::from(&call_id);
    let session_id = match resolve_session_id(input.session_id.as_deref()) {
        Ok(value) => value,
        Err(error) => {
            return Ok(ToolResult::error(call_id, "js_repl", error).with_call_id(external_call_id));
        }
    };
    let Some(session) = get_session(&session_id) else {
        return Ok(ToolResult::error(
            call_id,
            "js_repl",
            format!("Unknown js_repl session_id `{session_id}`"),
        )
        .with_call_id(external_call_id));
    };

    session.touch();
    let cleared = session.clear();

    Ok(ToolResult {
        id: call_id,
        call_id: external_call_id,
        tool_name: "js_repl".into(),
        parts: vec![MessagePart::text(format!(
            "[js_repl mode=reset]\nsession_id> {}\nstate> ready\ncleared_snippet_count> {}",
            session.id, cleared
        ))],
        structured_content: Some(
            serde_json::to_value(JsReplToolOutput::Reset {
                session_id: session.id.as_str().to_string(),
                state: "ready".to_string(),
                cleared_snippet_count: cleared,
            })
            .expect("js_repl reset output"),
        ),
        metadata: Some(serde_json::json!({
            "mode": "reset",
            "session_id": session.id.as_str(),
            "state": "ready",
            "cleared_snippet_count": cleared,
        })),
        is_error: false,
    })
}

async fn execute_close(call_id: ToolCallId, input: JsReplToolInput) -> Result<ToolResult> {
    let external_call_id = types::CallId::from(&call_id);
    let session_id = match resolve_session_id(input.session_id.as_deref()) {
        Ok(value) => value,
        Err(error) => {
            return Ok(ToolResult::error(call_id, "js_repl", error).with_call_id(external_call_id));
        }
    };
    let closed = remove_session(&session_id).is_some();

    Ok(ToolResult {
        id: call_id,
        call_id: external_call_id,
        tool_name: "js_repl".into(),
        parts: vec![MessagePart::text(format!(
            "[js_repl mode=close]\nsession_id> {}\nstate> {}\nclosed> {}",
            session_id,
            if closed { "closed" } else { "missing" },
            closed
        ))],
        structured_content: Some(
            serde_json::to_value(JsReplToolOutput::Close {
                session_id: session_id.as_str().to_string(),
                state: if closed { "closed" } else { "missing" }.to_string(),
                closed,
            })
            .expect("js_repl close output"),
        ),
        metadata: Some(serde_json::json!({
            "mode": "close",
            "session_id": session_id.as_str(),
            "state": if closed { "closed" } else { "missing" },
            "closed": closed,
        })),
        is_error: false,
    })
}

fn evaluate_javascript(
    history: Vec<String>,
    code: String,
    timeout_ms: u64,
    max_output_chars: usize,
) -> std::result::Result<JsEvalOutcome, JsEvalFailure> {
    let runtime = Runtime::new().map_err(|error| JsEvalFailure {
        message: format!("failed to create QuickJS runtime: {error}"),
        logs: Vec::new(),
        output_truncated: false,
        timed_out: false,
    })?;
    runtime.set_memory_limit(JS_RUNTIME_MEMORY_LIMIT_BYTES);
    runtime.set_max_stack_size(JS_RUNTIME_STACK_LIMIT_BYTES);
    runtime.set_gc_threshold(JS_RUNTIME_GC_THRESHOLD_BYTES);

    let timed_out = Arc::new(AtomicBool::new(false));
    let started_at = Instant::now();
    runtime.set_interrupt_handler(Some(Box::new({
        let timed_out = timed_out.clone();
        move || {
            let expired = started_at.elapsed() >= Duration::from_millis(timeout_ms);
            if expired {
                timed_out.store(true, Ordering::SeqCst);
            }
            expired
        }
    })));

    let context = Context::builder()
        .with::<ReplIntrinsics>()
        .build(&runtime)
        .map_err(|error| JsEvalFailure {
            message: format!("failed to create QuickJS context: {error}"),
            logs: Vec::new(),
            output_truncated: false,
            timed_out: false,
        })?;

    let capture = Arc::new(Mutex::new(OutputCapture::new(max_output_chars)));
    context.with(|ctx| {
        evaluate_in_context(
            ctx,
            &history,
            &code,
            timeout_ms,
            max_output_chars,
            &timed_out,
            capture,
        )
    })
}

fn evaluate_in_context(
    ctx: Ctx<'_>,
    history: &[String],
    code: &str,
    timeout_ms: u64,
    max_output_chars: usize,
    timed_out: &Arc<AtomicBool>,
    capture: Arc<Mutex<OutputCapture>>,
) -> std::result::Result<JsEvalOutcome, JsEvalFailure> {
    install_controlled_console(ctx.clone(), capture.clone()).map_err(|error| JsEvalFailure {
        message: format!(
            "failed to install js_repl console: {}",
            match error {
                rquickjs::Error::Exception => format_exception(&ctx),
                other => other.to_string(),
            }
        ),
        logs: Vec::new(),
        output_truncated: false,
        timed_out: false,
    })?;
    remove_restricted_globals(&ctx).map_err(|error| JsEvalFailure {
        message: format!(
            "failed to restrict js_repl globals: {}",
            match error {
                rquickjs::Error::Exception => format_exception(&ctx),
                other => other.to_string(),
            }
        ),
        logs: Vec::new(),
        output_truncated: false,
        timed_out: false,
    })?;

    {
        let mut output = capture.lock().expect("js_repl capture lock");
        output.suppressed = true;
    }
    for (index, snippet) in history.iter().enumerate() {
        if let Err(error) = execute_script(ctx.clone(), snippet, timeout_ms, timed_out) {
            return Err(JsEvalFailure {
                message: format!(
                    "session replay failed at snippet {}: {}",
                    index + 1,
                    error.message
                ),
                logs: Vec::new(),
                output_truncated: false,
                timed_out: error.timed_out,
            });
        }
    }
    {
        let mut output = capture.lock().expect("js_repl capture lock");
        output.suppressed = false;
    }

    let result = execute_script(ctx.clone(), code, timeout_ms, timed_out).map_err(|error| {
        let output = capture.lock().expect("js_repl capture lock");
        JsEvalFailure {
            message: error.message,
            logs: output.logs.clone(),
            output_truncated: output.truncated,
            timed_out: error.timed_out,
        }
    })?;
    let rendered = render_js_result(&ctx, result, max_output_chars).map_err(|error| {
        let output = capture.lock().expect("js_repl capture lock");
        JsEvalFailure {
            message: format!("failed to render JavaScript result: {error}"),
            logs: output.logs.clone(),
            output_truncated: output.truncated,
            timed_out: false,
        }
    })?;
    let output = capture.lock().expect("js_repl capture lock");

    Ok(JsEvalOutcome {
        result: rendered,
        logs: output.logs.clone(),
        output_truncated: output.truncated,
    })
}

fn install_controlled_console(
    ctx: Ctx<'_>,
    capture: Arc<Mutex<OutputCapture>>,
) -> rquickjs::Result<()> {
    let console = Object::new(ctx.clone())?;
    console.set(
        "log",
        Func::from(MutFn::from({
            let capture = capture.clone();
            move |args: Rest<Coerced<String>>| {
                let text = args
                    .iter()
                    .map(|value| value.0.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                capture
                    .lock()
                    .expect("js_repl capture lock")
                    .push("log", text);
            }
        })),
    )?;
    console.set(
        "info",
        Func::from(MutFn::from({
            let capture = capture.clone();
            move |args: Rest<Coerced<String>>| {
                let text = args
                    .iter()
                    .map(|value| value.0.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                capture
                    .lock()
                    .expect("js_repl capture lock")
                    .push("info", text);
            }
        })),
    )?;
    console.set(
        "warn",
        Func::from(MutFn::from({
            let capture = capture.clone();
            move |args: Rest<Coerced<String>>| {
                let text = args
                    .iter()
                    .map(|value| value.0.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                capture
                    .lock()
                    .expect("js_repl capture lock")
                    .push("warn", text);
            }
        })),
    )?;
    console.set(
        "error",
        Func::from(MutFn::from({
            let capture = capture.clone();
            move |args: Rest<Coerced<String>>| {
                let text = args
                    .iter()
                    .map(|value| value.0.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                capture
                    .lock()
                    .expect("js_repl capture lock")
                    .push("error", text);
            }
        })),
    )?;

    // Install a host-owned console object before user code runs. The bridge
    // intentionally accepts only string-coerced arguments so the transcript
    // surface stays deterministic and free from host object references.
    ctx.globals().set("console", console)?;
    Ok(())
}

fn remove_restricted_globals(ctx: &Ctx<'_>) -> rquickjs::Result<()> {
    let globals = ctx.globals();
    for name in RESTRICTED_GLOBALS {
        if globals.contains_key(*name)? {
            globals.remove(*name)?;
        }
    }
    Ok(())
}

fn execute_script<'js>(
    ctx: Ctx<'js>,
    source: &str,
    timeout_ms: u64,
    timed_out: &Arc<AtomicBool>,
) -> std::result::Result<JsValue<'js>, JsEvalFailure> {
    if source.contains("await") {
        let promise = ctx
            .eval_promise(source)
            .map_err(|error| describe_js_error(&ctx, error, timeout_ms, timed_out))?;
        let value = promise
            .finish::<JsValue<'_>>()
            .map_err(|error| describe_js_error(&ctx, error, timeout_ms, timed_out))?;
        return unwrap_async_eval_result(value)
            .map_err(|error| describe_js_error(&ctx, error, timeout_ms, timed_out));
    }

    ctx.eval::<JsValue<'_>, _>(source)
        .map_err(|error| describe_js_error(&ctx, error, timeout_ms, timed_out))
}

fn unwrap_async_eval_result<'js>(value: JsValue<'js>) -> rquickjs::Result<JsValue<'js>> {
    let Some(object) = value.clone().into_object() else {
        return Ok(value);
    };

    let mut keys = object.keys::<String>();
    let Some(first_key) = keys.next() else {
        return Ok(value);
    };
    let first_key = first_key?;
    if first_key != "value" || keys.next().transpose()?.is_some() {
        return Ok(value);
    }
    object.get::<_, JsValue<'_>>("value").or(Ok(value))
}

fn describe_js_error(
    ctx: &Ctx<'_>,
    error: rquickjs::Error,
    timeout_ms: u64,
    timed_out: &Arc<AtomicBool>,
) -> JsEvalFailure {
    if timed_out.load(Ordering::SeqCst) {
        return JsEvalFailure {
            message: format!("script execution timed out after {timeout_ms}ms"),
            logs: Vec::new(),
            output_truncated: false,
            timed_out: true,
        };
    }
    let message = match error {
        rquickjs::Error::Exception => format_exception(ctx),
        rquickjs::Error::WouldBlock => {
            "evaluation returned a pending promise that cannot be driven without host async APIs"
                .to_string()
        }
        other => other.to_string(),
    };
    JsEvalFailure {
        message,
        logs: Vec::new(),
        output_truncated: false,
        timed_out: false,
    }
}

fn format_exception(ctx: &Ctx<'_>) -> String {
    let caught = ctx.catch();
    if let Some(object) = caught.clone().into_object() {
        if let Some(exception) = Exception::from_object(object) {
            let name = exception
                .get::<_, Option<Coerced<String>>>("name")
                .ok()
                .flatten()
                .map(|value| value.0);
            let message = exception.message().map(|message| match &name {
                Some(name) if !message.starts_with(name) => format!("{name}: {message}"),
                _ => message,
            });
            match (message, exception.stack()) {
                (Some(message), Some(stack)) if stack.contains(&message) => return stack,
                (Some(message), Some(stack)) => return format!("{message}\n{stack}"),
                (Some(message), None) => return message,
                (None, Some(stack)) => return stack,
                (None, None) => {}
            }
        }
    }
    render_js_value_text(ctx, caught).unwrap_or_else(|error| error.to_string())
}

fn render_js_result<'js>(
    ctx: &Ctx<'js>,
    value: JsValue<'js>,
    max_output_chars: usize,
) -> rquickjs::Result<JsReplResultOutput> {
    let type_name = value.type_name().to_string();
    let json_text = ctx
        .json_stringify(value.clone())
        .ok()
        .flatten()
        .and_then(|value| value.to_string().ok());
    let preview_source = json_text
        .clone()
        .unwrap_or(render_js_value_text(ctx, value.clone())?);
    let (preview, preview_truncated) = truncate_text_with_limit(&preview_source, max_output_chars);
    let json = json_text
        .filter(|text| text.chars().count() <= max_output_chars)
        .and_then(|text| serde_json::from_str(&text).ok());
    Ok(JsReplResultOutput {
        type_name,
        preview,
        preview_truncated,
        json,
    })
}

fn render_js_value_text<'js>(ctx: &Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<String> {
    if let Ok(Some(json)) = ctx.json_stringify(value.clone()) {
        if let Ok(text) = json.to_string() {
            return Ok(text);
        }
    }
    Ok(Coerced::<String>::from_js(ctx, value)?.0)
}

fn render_eval_output(
    session_id: Option<&str>,
    persisted: bool,
    timeout_ms: u64,
    result: &JsReplResultOutput,
    logs: &[JsReplLogOutput],
    output_truncated: bool,
) -> String {
    let mut sections = vec![format!(
        "[js_repl mode=eval persisted={} timeout_ms={}]",
        persisted, timeout_ms
    )];
    if let Some(session_id) = session_id {
        sections.push(format!("session_id> {session_id}"));
    }
    sections.push(format!("result_type> {}", result.type_name));
    sections.push(format!("result_preview> {}", result.preview));
    if let Some(json) = &result.json {
        sections.push(format!("result_json> {}", json));
    }
    if !logs.is_empty() {
        let console = logs
            .iter()
            .map(|entry| format!("[{}] {}", entry.level, entry.text))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("console>\n{console}"));
    }
    if result.preview_truncated || output_truncated {
        sections.push("[output truncated to fit max_output_chars]".to_string());
    }
    sections.join("\n\n")
}

fn render_error_output(
    session_id: Option<&str>,
    persisted: bool,
    timeout_ms: u64,
    error: &str,
    logs: &[JsReplLogOutput],
    output_truncated: bool,
    timed_out: bool,
) -> String {
    let mut sections = vec![format!(
        "[js_repl mode=eval persisted={} timeout_ms={}]",
        persisted, timeout_ms
    )];
    if let Some(session_id) = session_id {
        sections.push(format!("session_id> {session_id}"));
    }
    if timed_out {
        sections.push("timed_out> true".to_string());
    }
    sections.push(format!("error> {error}"));
    if !logs.is_empty() {
        let console = logs
            .iter()
            .map(|entry| format!("[{}] {}", entry.level, entry.text))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("console>\n{console}"));
    }
    if output_truncated {
        sections.push("[output truncated to fit max_output_chars]".to_string());
    }
    sections.join("\n\n")
}

fn resolve_code(input: &JsReplToolInput) -> Result<String> {
    let Some(code) = input.code.as_ref() else {
        return Err(ToolError::invalid(
            "js_repl requires a non-empty `code` field for eval mode",
        ));
    };
    let trimmed = code.trim();
    if trimmed.is_empty() {
        return Err(ToolError::invalid(
            "js_repl requires a non-empty `code` field for eval mode",
        ));
    }
    if trimmed.chars().count() > MAX_CODE_CHARS {
        return Err(ToolError::invalid(format!(
            "js_repl code exceeds the {MAX_CODE_CHARS}-character limit"
        )));
    }
    Ok(trimmed.to_string())
}

fn resolve_session_id(session_id: Option<&str>) -> std::result::Result<JsReplSessionId, String> {
    session_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(JsReplSessionId::from)
        .ok_or_else(|| "js_repl requires a non-empty `session_id` for this mode".to_string())
}

fn truncate_text_with_limit(value: &str, limit: usize) -> (String, bool) {
    let char_count = value.chars().count();
    if char_count <= limit {
        return (value.to_string(), false);
    }
    (value.chars().take(limit).collect::<String>(), true)
}

fn unix_timestamp_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn sessions() -> &'static SessionRegistry {
    JS_REPL_SESSIONS.get_or_init(SessionRegistry::new)
}

fn get_session(session_id: &JsReplSessionId) -> Option<Arc<JsReplSession>> {
    sessions()
        .get(session_id)
        .map(|entry| Arc::clone(entry.value()))
}

fn insert_session(session: Arc<JsReplSession>) {
    let registry = sessions();
    prune_sessions(registry);
    registry.insert(session.id.clone(), session);
}

fn remove_session(session_id: &JsReplSessionId) -> Option<Arc<JsReplSession>> {
    sessions().remove(session_id).map(|(_, session)| session)
}

fn prune_sessions(registry: &SessionRegistry) {
    if registry.len() < MAX_TRACKED_JS_REPL_SESSIONS {
        return;
    }

    let mut ordered = registry
        .iter()
        .map(|entry| (entry.key().clone(), entry.value().last_used_at_unix_s()))
        .collect::<Vec<_>>();
    ordered.sort_by_key(|(_, last_used)| *last_used);

    let remove_count = registry.len().saturating_sub(MAX_TRACKED_JS_REPL_SESSIONS) + 1;
    for (session_id, _) in ordered.into_iter().take(remove_count) {
        registry.remove(&session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::{JsReplMode, JsReplTool, JsReplToolInput};
    use crate::{Tool, ToolExecutionContext};
    use types::ToolCallId;

    async fn execute(input: JsReplToolInput) -> types::ToolResult {
        JsReplTool::new()
            .execute(
                ToolCallId::new(),
                serde_json::to_value(input).unwrap(),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn js_repl_evaluates_javascript_and_captures_console_output() {
        let result = execute(JsReplToolInput {
            mode: Some(JsReplMode::Eval),
            code: Some("console.log('hi'); ({ answer: 42 })".to_string()),
            session_id: None,
            timeout_ms: Some(1_000),
            max_output_chars: Some(1_024),
        })
        .await;

        assert!(!result.is_error);
        assert!(result.text_content().contains("result_type> object"));
        assert!(result.text_content().contains("[log] hi"));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["kind"], "eval");
        assert_eq!(structured["result"]["json"]["answer"], 42);
    }

    #[tokio::test]
    async fn js_repl_sessions_replay_state_without_replaying_console_output() {
        let start = execute(JsReplToolInput {
            mode: Some(JsReplMode::Start),
            code: None,
            session_id: None,
            timeout_ms: None,
            max_output_chars: None,
        })
        .await;
        let session_id = start.structured_content.unwrap()["session_id"]
            .as_str()
            .unwrap()
            .to_string();

        let first = execute(JsReplToolInput {
            mode: Some(JsReplMode::Eval),
            code: Some("console.log('boot'); let counter = 2;".to_string()),
            session_id: Some(session_id.clone()),
            timeout_ms: Some(1_000),
            max_output_chars: Some(1_024),
        })
        .await;
        assert!(!first.is_error);
        assert!(first.text_content().contains("[log] boot"));

        let second = execute(JsReplToolInput {
            mode: Some(JsReplMode::Eval),
            code: Some("counter + 3".to_string()),
            session_id: Some(session_id),
            timeout_ms: Some(1_000),
            max_output_chars: Some(1_024),
        })
        .await;
        assert!(!second.is_error);
        assert!(second.text_content().contains("result_preview> 5"));
        assert!(!second.text_content().contains("boot"));
    }

    #[tokio::test]
    async fn js_repl_reset_clears_persisted_history() {
        let start = execute(JsReplToolInput {
            mode: Some(JsReplMode::Start),
            code: None,
            session_id: None,
            timeout_ms: None,
            max_output_chars: None,
        })
        .await;
        let session_id = start.structured_content.unwrap()["session_id"]
            .as_str()
            .unwrap()
            .to_string();

        let _ = execute(JsReplToolInput {
            mode: Some(JsReplMode::Eval),
            code: Some("let token = 7;".to_string()),
            session_id: Some(session_id.clone()),
            timeout_ms: Some(1_000),
            max_output_chars: Some(1_024),
        })
        .await;

        let reset = execute(JsReplToolInput {
            mode: Some(JsReplMode::Reset),
            code: None,
            session_id: Some(session_id.clone()),
            timeout_ms: None,
            max_output_chars: None,
        })
        .await;
        assert!(!reset.is_error);
        assert!(reset.text_content().contains("cleared_snippet_count> 1"));

        let eval = execute(JsReplToolInput {
            mode: Some(JsReplMode::Eval),
            code: Some("token".to_string()),
            session_id: Some(session_id),
            timeout_ms: Some(1_000),
            max_output_chars: Some(1_024),
        })
        .await;
        assert!(eval.is_error);
        assert!(eval.text_content().contains("ReferenceError"));
    }

    #[tokio::test]
    async fn js_repl_times_out_infinite_loops() {
        let result = execute(JsReplToolInput {
            mode: Some(JsReplMode::Eval),
            code: Some("while (true) {}".to_string()),
            session_id: None,
            timeout_ms: Some(25),
            max_output_chars: Some(1_024),
        })
        .await;

        assert!(result.is_error);
        assert!(result.text_content().contains("timed_out> true"));
        assert!(result.text_content().contains("timed out after 25ms"));
    }

    #[tokio::test]
    async fn js_repl_disables_string_code_generation_primitives() {
        let result = execute(JsReplToolInput {
            mode: Some(JsReplMode::Eval),
            code: Some("Function('return 1')()".to_string()),
            session_id: None,
            timeout_ms: Some(1_000),
            max_output_chars: Some(1_024),
        })
        .await;

        assert!(result.is_error);
        assert!(result.text_content().contains("ReferenceError"));
    }

    #[tokio::test]
    async fn js_repl_supports_top_level_await() {
        let result = execute(JsReplToolInput {
            mode: Some(JsReplMode::Eval),
            code: Some("await Promise.resolve(9 * 9)".to_string()),
            session_id: None,
            timeout_ms: Some(1_000),
            max_output_chars: Some(1_024),
        })
        .await;

        assert!(!result.is_error);
        assert!(result.text_content().contains("result_preview> 81"));
    }
}
