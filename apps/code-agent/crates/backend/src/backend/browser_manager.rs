use crate::backend::SessionEventStream;
use crate::ui::SessionEvent;
use agent::tools::{
    BrowserClickRequest, BrowserCloseRequest, BrowserEvalRequest, BrowserManager,
    BrowserOpenRequest, BrowserRuntimeContext, BrowserSnapshotElement, BrowserSnapshotElementKind,
    BrowserSnapshotRecord, BrowserSnapshotRequest, BrowserTypeRequest, Result as ToolResult,
    ToolError,
};
use agent::types::{
    BrowserId, BrowserStatus, BrowserSummaryRecord, SessionEventEnvelope, SessionEventKind,
    SessionId, new_opaque_id,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use store::SessionStore;

trait BrowserHandle: Send + Sync {
    fn snapshot(&self, request: &BrowserSnapshotRequest) -> ToolResult<BrowserSnapshotRecord>;
    fn click(
        &self,
        selector: &str,
        wait_for_navigation: bool,
    ) -> ToolResult<BrowserInteractionOutcome>;
    fn type_text(&self, request: &BrowserTypeRequest) -> ToolResult<BrowserInteractionOutcome>;
    fn eval(&self, request: &BrowserEvalRequest) -> ToolResult<BrowserEvalOutcome>;
    fn close(&self, fire_unload: bool) -> ToolResult<()>;
}

struct BrowserLaunch {
    handle: Arc<dyn BrowserHandle>,
    current_url: String,
    title: Option<String>,
}

struct BrowserInteractionOutcome {
    current_url: String,
    title: Option<String>,
}

struct BrowserEvalOutcome {
    browser: BrowserInteractionOutcome,
    result: Value,
}

trait BrowserBackend: Send + Sync {
    fn open(&self, request: BrowserOpenRequest) -> ToolResult<BrowserLaunch>;
}

struct SessionBrowser {
    summary: Mutex<BrowserSummaryRecord>,
    _handle: Arc<dyn BrowserHandle>,
}

impl SessionBrowser {
    fn summary(&self) -> BrowserSummaryRecord {
        self.summary.lock().expect("browser summary lock").clone()
    }

    fn update_summary(
        &self,
        apply: impl FnOnce(&mut BrowserSummaryRecord),
    ) -> BrowserSummaryRecord {
        let mut summary = self.summary.lock().expect("browser summary lock");
        apply(&mut summary);
        summary.clone()
    }
}

#[derive(Clone)]
pub struct SessionBrowserManager {
    store: Arc<dyn SessionStore>,
    events: SessionEventStream,
    backend: Arc<dyn BrowserBackend>,
    browsers: Arc<Mutex<BTreeMap<BrowserId, Arc<SessionBrowser>>>>,
}

impl SessionBrowserManager {
    #[must_use]
    pub fn new(store: Arc<dyn SessionStore>, events: SessionEventStream) -> Self {
        Self::with_backend(store, events, Arc::new(HeadlessChromeBrowserBackend))
    }

    fn with_backend(
        store: Arc<dyn SessionStore>,
        events: SessionEventStream,
        backend: Arc<dyn BrowserBackend>,
    ) -> Self {
        Self {
            store,
            events,
            backend,
            browsers: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    fn require_attached_runtime(
        runtime: &BrowserRuntimeContext,
    ) -> ToolResult<(SessionId, agent::types::AgentSessionId)> {
        let session_id = runtime.session_id.clone().ok_or_else(|| {
            ToolError::invalid_state("browser tools require an attached runtime session")
        })?;
        let agent_session_id = runtime.agent_session_id.clone().ok_or_else(|| {
            ToolError::invalid_state("browser tools require an attached runtime agent session")
        })?;
        Ok((session_id, agent_session_id))
    }

    async fn append_session_event(
        &self,
        runtime: &BrowserRuntimeContext,
        event: SessionEventKind,
    ) -> ToolResult<()> {
        let (session_id, agent_session_id) = Self::require_attached_runtime(runtime)?;
        self.store
            .append(SessionEventEnvelope::new(
                session_id,
                agent_session_id,
                runtime.turn_id.clone(),
                None,
                event,
            ))
            .await
            .map_err(|error| ToolError::invalid_state(error.to_string()))
    }

    async fn publish_opened(
        &self,
        runtime: &BrowserRuntimeContext,
        summary: BrowserSummaryRecord,
    ) -> ToolResult<()> {
        self.append_session_event(
            runtime,
            SessionEventKind::BrowserOpened {
                summary: summary.clone(),
            },
        )
        .await?;
        self.events.publish(SessionEvent::BrowserOpened { summary });
        Ok(())
    }

    async fn publish_updated(
        &self,
        runtime: &BrowserRuntimeContext,
        summary: BrowserSummaryRecord,
    ) -> ToolResult<()> {
        self.append_session_event(
            runtime,
            SessionEventKind::BrowserUpdated {
                summary: summary.clone(),
            },
        )
        .await?;
        self.events
            .publish(SessionEvent::BrowserUpdated { summary });
        Ok(())
    }

    fn insert_browser(&self, state: Arc<SessionBrowser>) {
        self.browsers
            .lock()
            .expect("browser registry lock")
            .insert(state.summary().browser_id.clone(), state);
    }

    fn remove_browser(&self, browser_id: &BrowserId) {
        self.browsers
            .lock()
            .expect("browser registry lock")
            .remove(browser_id);
    }

    async fn apply_interaction_outcome(
        &self,
        runtime: &BrowserRuntimeContext,
        state: Arc<SessionBrowser>,
        outcome: BrowserInteractionOutcome,
    ) -> ToolResult<BrowserSummaryRecord> {
        let summary = state.update_summary(|summary| {
            summary.current_url = outcome.current_url;
            summary.title = outcome.title;
            summary.updated_at_unix_s = Some(unix_timestamp_s());
        });
        self.publish_updated(runtime, summary.clone()).await?;
        Ok(summary)
    }

    fn resolve_browser_state(
        &self,
        runtime: &BrowserRuntimeContext,
        browser_id: Option<&BrowserId>,
    ) -> ToolResult<Arc<SessionBrowser>> {
        let (session_id, _) = Self::require_attached_runtime(runtime)?;
        let browsers = self.browsers.lock().expect("browser registry lock");
        if let Some(browser_id) = browser_id {
            let state = browsers.get(browser_id).cloned().ok_or_else(|| {
                ToolError::invalid_state(format!("browser {browser_id} is not open"))
            })?;
            if state.summary().session_id != session_id {
                return Err(ToolError::invalid_state(format!(
                    "browser {browser_id} is not attached to the current session"
                )));
            }
            return Ok(state);
        }

        let mut attached = browsers
            .values()
            .filter(|state| state.summary().session_id == session_id)
            .cloned()
            .collect::<Vec<_>>();
        match attached.len() {
            0 => Err(ToolError::invalid_state(
                "browser tools require an open browser session",
            )),
            1 => Ok(attached.remove(0)),
            _ => Err(ToolError::invalid(
                "browser tools require browser_id when multiple browsers are open",
            )),
        }
    }
}

#[async_trait]
impl BrowserManager for SessionBrowserManager {
    async fn open_browser(
        &self,
        runtime: BrowserRuntimeContext,
        request: BrowserOpenRequest,
    ) -> ToolResult<BrowserSummaryRecord> {
        let _ = Self::require_attached_runtime(&runtime)?;
        let requested_headless = request.headless;
        let requested_viewport = request.viewport.clone();
        let backend = self.backend.clone();
        let launch = tokio::task::spawn_blocking(move || backend.open(request))
            .await
            .map_err(|error| {
                ToolError::invalid_state(format!("failed to join browser task: {error}"))
            })??;
        let summary = BrowserSummaryRecord {
            browser_id: BrowserId::from(format!("browser_{}", new_opaque_id())),
            session_id: runtime
                .session_id
                .clone()
                .expect("browser summary requires session_id"),
            agent_session_id: runtime
                .agent_session_id
                .clone()
                .expect("browser summary requires agent_session_id"),
            parent_agent_id: runtime.parent_agent_id.clone(),
            task_id: runtime.task_id.clone(),
            status: BrowserStatus::Open,
            current_url: launch.current_url,
            headless: requested_headless,
            title: launch.title,
            viewport: requested_viewport,
            opened_at_unix_s: unix_timestamp_s(),
            updated_at_unix_s: None,
            closed_at_unix_s: None,
        };
        let state = Arc::new(SessionBrowser {
            summary: Mutex::new(summary.clone()),
            _handle: launch.handle,
        });
        self.insert_browser(state);
        self.publish_opened(&runtime, summary.clone()).await?;
        Ok(summary)
    }

    async fn snapshot_browser(
        &self,
        runtime: BrowserRuntimeContext,
        request: BrowserSnapshotRequest,
    ) -> ToolResult<BrowserSnapshotRecord> {
        let state = self.resolve_browser_state(&runtime, request.browser_id.as_ref())?;
        let summary = state.summary();
        let browser_id = summary.browser_id.clone();
        let handle = state._handle.clone();
        let snapshot = tokio::task::spawn_blocking(move || handle.snapshot(&request))
            .await
            .map_err(|error| {
                ToolError::invalid_state(format!("failed to join browser task: {error}"))
            })??;
        Ok(BrowserSnapshotRecord {
            browser_id,
            ..snapshot
        })
    }

    async fn click_browser(
        &self,
        runtime: BrowserRuntimeContext,
        request: BrowserClickRequest,
    ) -> ToolResult<BrowserSummaryRecord> {
        let state = self.resolve_browser_state(&runtime, request.browser_id.as_ref())?;
        let handle = state._handle.clone();
        let selector = request.selector.clone();
        let wait_for_navigation = request.wait_for_navigation;
        let click =
            tokio::task::spawn_blocking(move || handle.click(&selector, wait_for_navigation))
                .await
                .map_err(|error| {
                    ToolError::invalid_state(format!("failed to join browser task: {error}"))
                })??;
        self.apply_interaction_outcome(&runtime, state, click).await
    }

    async fn type_browser(
        &self,
        runtime: BrowserRuntimeContext,
        request: BrowserTypeRequest,
    ) -> ToolResult<BrowserSummaryRecord> {
        let state = self.resolve_browser_state(&runtime, request.browser_id.as_ref())?;
        let handle = state._handle.clone();
        let typed = tokio::task::spawn_blocking(move || handle.type_text(&request))
            .await
            .map_err(|error| {
                ToolError::invalid_state(format!("failed to join browser task: {error}"))
            })??;
        self.apply_interaction_outcome(&runtime, state, typed).await
    }

    async fn eval_browser(
        &self,
        runtime: BrowserRuntimeContext,
        request: BrowserEvalRequest,
    ) -> ToolResult<(BrowserSummaryRecord, Value)> {
        let state = self.resolve_browser_state(&runtime, request.browser_id.as_ref())?;
        let handle = state._handle.clone();
        let evaluated = tokio::task::spawn_blocking(move || handle.eval(&request))
            .await
            .map_err(|error| {
                ToolError::invalid_state(format!("failed to join browser task: {error}"))
            })??;
        let summary = self
            .apply_interaction_outcome(&runtime, state, evaluated.browser)
            .await?;
        Ok((summary, evaluated.result))
    }

    async fn close_browser(
        &self,
        runtime: BrowserRuntimeContext,
        request: BrowserCloseRequest,
    ) -> ToolResult<BrowserSummaryRecord> {
        let state = self.resolve_browser_state(&runtime, request.browser_id.as_ref())?;
        let browser_id = state.summary().browser_id.clone();
        let handle = state._handle.clone();
        tokio::task::spawn_blocking(move || handle.close(request.fire_unload))
            .await
            .map_err(|error| {
                ToolError::invalid_state(format!("failed to join browser task: {error}"))
            })??;

        // Closing the session must also detach it from the live registry so
        // later browser tools cannot accidentally reuse a stale Chromium tab.
        self.remove_browser(&browser_id);
        let closed_at = unix_timestamp_s();
        let summary = state.update_summary(|summary| {
            summary.status = BrowserStatus::Closed;
            summary.updated_at_unix_s = Some(closed_at);
            summary.closed_at_unix_s = Some(closed_at);
        });
        self.publish_updated(&runtime, summary.clone()).await?;
        Ok(summary)
    }
}

fn unix_timestamp_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |value| value.as_secs())
}

// The browser lifecycle is a host-owned optional surface. Chromium's own
// sandbox regularly refuses to start inside rootful/containerized developer
// environments, so the backend disables that inner sandbox explicitly and
// relies on the outer host/session approval boundary instead.
struct HeadlessChromeBrowserBackend;

impl BrowserBackend for HeadlessChromeBrowserBackend {
    fn open(&self, request: BrowserOpenRequest) -> ToolResult<BrowserLaunch> {
        use headless_chrome::Browser;
        use headless_chrome::LaunchOptions;

        let mut builder = LaunchOptions::default_builder();
        builder.headless(request.headless);
        builder.sandbox(false);
        if let Some(viewport) = request.viewport.as_ref() {
            builder.window_size(Some((viewport.width, viewport.height)));
        }
        let browser = Browser::new(builder.build().map_err(|error| {
            ToolError::invalid_state(format!("invalid browser launch options: {error}"))
        })?)
        .map_err(|error| ToolError::invalid_state(format!("failed to launch browser: {error}")))?;
        let tab = browser.new_tab().map_err(|error| {
            ToolError::invalid_state(format!("failed to create browser tab: {error}"))
        })?;
        tab.navigate_to(&request.url)
            .and_then(|tab| tab.wait_until_navigated())
            .map_err(|error| {
                ToolError::invalid_state(format!("failed to navigate browser: {error}"))
            })?;
        let current_url = tab.get_url();
        let title = tab
            .get_title()
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        Ok(BrowserLaunch {
            handle: Arc::new(HeadlessChromeBrowserHandle {
                _browser: browser,
                _tab: tab,
            }),
            current_url,
            title,
        })
    }
}

struct HeadlessChromeBrowserHandle {
    _browser: headless_chrome::Browser,
    _tab: Arc<headless_chrome::Tab>,
}

impl BrowserHandle for HeadlessChromeBrowserHandle {
    fn snapshot(&self, request: &BrowserSnapshotRequest) -> ToolResult<BrowserSnapshotRecord> {
        let current_url = self._tab.get_url();
        let title = self
            ._tab
            .get_title()
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let text_preview = extract_text_preview(&self._tab, request.max_text_lines)?;
        let interactive_elements = extract_interactive_elements(&self._tab, request.max_elements)?;
        let html_preview = if request.include_html {
            html_preview_lines(
                &self._tab.get_content().map_err(|error| {
                    ToolError::invalid_state(format!("failed to read browser html: {error}"))
                })?,
                request.max_html_chars,
            )
        } else {
            Vec::new()
        };

        Ok(BrowserSnapshotRecord {
            browser_id: BrowserId::from("browser_pending"),
            current_url,
            title,
            text_preview,
            interactive_elements,
            html_preview,
        })
    }

    fn click(
        &self,
        selector: &str,
        wait_for_navigation: bool,
    ) -> ToolResult<BrowserInteractionOutcome> {
        self._tab
            .find_element(selector)
            .map_err(|error| {
                ToolError::invalid_state(format!(
                    "failed to resolve browser selector {selector}: {error}"
                ))
            })?
            .click()
            .map_err(|error| {
                ToolError::invalid_state(format!(
                    "failed to click browser selector {selector}: {error}"
                ))
            })?;
        if wait_for_navigation {
            self._tab.wait_until_navigated().map_err(|error| {
                ToolError::invalid_state(format!(
                    "browser navigation did not complete after clicking {selector}: {error}"
                ))
            })?;
        }
        Ok(BrowserInteractionOutcome {
            current_url: self._tab.get_url(),
            title: self
                ._tab
                .get_title()
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        })
    }

    fn type_text(&self, request: &BrowserTypeRequest) -> ToolResult<BrowserInteractionOutcome> {
        let element = self._tab.find_element(&request.selector).map_err(|error| {
            ToolError::invalid_state(format!(
                "failed to resolve browser selector {}: {error}",
                request.selector
            ))
        })?;
        if request.clear_first {
            element
                .call_js_fn(
                    "function() { if ('value' in this) { this.value = ''; this.dispatchEvent(new Event('input', { bubbles: true })); this.dispatchEvent(new Event('change', { bubbles: true })); } return true; }",
                    vec![],
                    false,
                )
                .map_err(|error| {
                    ToolError::invalid_state(format!(
                        "failed to clear browser selector {}: {error}",
                        request.selector
                    ))
                })?;
        }
        element.type_into(&request.text).map_err(|error| {
            ToolError::invalid_state(format!(
                "failed to type into browser selector {}: {error}",
                request.selector
            ))
        })?;
        if request.submit {
            self._tab.press_key("Enter").map_err(|error| {
                ToolError::invalid_state(format!(
                    "failed to submit browser selector {}: {error}",
                    request.selector
                ))
            })?;
        }
        if request.wait_for_navigation {
            self._tab.wait_until_navigated().map_err(|error| {
                ToolError::invalid_state(format!(
                    "browser navigation did not complete after typing into {}: {error}",
                    request.selector
                ))
            })?;
        }
        Ok(BrowserInteractionOutcome {
            current_url: self._tab.get_url(),
            title: self
                ._tab
                .get_title()
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        })
    }

    fn eval(&self, request: &BrowserEvalRequest) -> ToolResult<BrowserEvalOutcome> {
        let result = self
            ._tab
            .evaluate(&request.script, request.await_promise)
            .map_err(|error| {
                ToolError::invalid_state(format!("failed to evaluate browser script: {error}"))
            })?;
        Ok(BrowserEvalOutcome {
            browser: BrowserInteractionOutcome {
                current_url: self._tab.get_url(),
                title: self
                    ._tab
                    .get_title()
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty()),
            },
            result: result
                .value
                .unwrap_or_else(|| json!("<non-serializable browser eval result>")),
        })
    }

    fn close(&self, fire_unload: bool) -> ToolResult<()> {
        self._tab.close(fire_unload).map(|_| ()).map_err(|error| {
            ToolError::invalid_state(format!("failed to close browser tab: {error}"))
        })
    }
}

#[derive(Deserialize)]
struct BrowserSnapshotPageData {
    #[serde(default)]
    text_preview: Vec<String>,
    #[serde(default)]
    interactive_elements: Vec<BrowserSnapshotElementData>,
}

#[derive(Deserialize)]
struct BrowserSnapshotElementData {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    selector_hint: Option<String>,
}

fn extract_text_preview(
    tab: &headless_chrome::Tab,
    max_text_lines: usize,
) -> ToolResult<Vec<String>> {
    let data = evaluate_snapshot_page_data(tab, max_text_lines, 0)?;
    Ok(data.text_preview)
}

fn extract_interactive_elements(
    tab: &headless_chrome::Tab,
    max_elements: usize,
) -> ToolResult<Vec<BrowserSnapshotElement>> {
    let data = evaluate_snapshot_page_data(tab, 0, max_elements)?;
    Ok(data
        .interactive_elements
        .into_iter()
        .map(BrowserSnapshotElementData::into_record)
        .collect())
}

fn evaluate_snapshot_page_data(
    tab: &headless_chrome::Tab,
    max_text_lines: usize,
    max_elements: usize,
) -> ToolResult<BrowserSnapshotPageData> {
    let script = format!(
        r#"
(() => {{
  const normalize = (value) => (value || "").replace(/\s+/g, " ").trim();
  const textPreview = (document.body?.innerText || "")
    .split(/\r?\n/)
    .map(normalize)
    .filter(Boolean)
    .slice(0, {max_text_lines});
  const interactiveElements = Array.from(
    document.querySelectorAll('a[href], button, input, textarea, select, [role="button"]')
  )
    .slice(0, {max_elements})
    .map((element) => {{
      const tag = (element.tagName || "").toLowerCase();
      const role = normalize(element.getAttribute('role'));
      const kind =
        tag === 'a' ? 'link' :
        tag === 'button' || role === 'button' ? 'button' :
        tag === 'input' ? 'input' :
        tag === 'textarea' ? 'textarea' :
        tag === 'select' ? 'select' :
        'other';
      const selectorHint = element.id
        ? `#${{element.id}}`
        : (() => {{
            const className = normalize(element.className);
            if (className) {{
              return `${{tag || 'node'}}.${{className.split(/\s+/)[0]}}`;
            }}
            return tag || null;
          }})();
      const text = normalize(
        tag === 'input'
          ? (element.getAttribute('value') || element.getAttribute('placeholder') || element.getAttribute('aria-label') || '')
          : (element.innerText || element.textContent || element.getAttribute('aria-label') || '')
      );
      const target = normalize(
        element.getAttribute('href') ||
        element.getAttribute('name') ||
        element.getAttribute('type') ||
        element.getAttribute('value') ||
        ''
      );
      return {{
        kind,
        text: text || null,
        target: target || null,
        selector_hint: selectorHint || null
      }};
    }});
  return {{
    text_preview: textPreview,
    interactive_elements: interactiveElements
  }};
}})()
"#
    );
    let value = tab
        .evaluate(&script, false)
        .map_err(|error| ToolError::invalid_state(format!("failed to inspect browser: {error}")))?
        .value
        .ok_or_else(|| ToolError::invalid_state("browser snapshot returned no value"))?;
    serde_json::from_value(value).map_err(|error| {
        ToolError::invalid_state(format!("invalid browser snapshot payload: {error}"))
    })
}

impl BrowserSnapshotElementData {
    fn into_record(self) -> BrowserSnapshotElement {
        BrowserSnapshotElement {
            kind: match self.kind.as_str() {
                "link" => BrowserSnapshotElementKind::Link,
                "button" => BrowserSnapshotElementKind::Button,
                "input" => BrowserSnapshotElementKind::Input,
                "textarea" => BrowserSnapshotElementKind::TextArea,
                "select" => BrowserSnapshotElementKind::Select,
                _ => BrowserSnapshotElementKind::Other,
            },
            text: self.text.filter(|value| !value.trim().is_empty()),
            target: self.target.filter(|value| !value.trim().is_empty()),
            selector_hint: self.selector_hint.filter(|value| !value.trim().is_empty()),
        }
    }
}

fn html_preview_lines(html: &str, max_chars: usize) -> Vec<String> {
    let truncated = html.chars().take(max_chars).collect::<String>();
    if truncated.is_empty() {
        return Vec::new();
    }
    truncated
        .chars()
        .collect::<Vec<_>>()
        .chunks(96)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use store::{InMemorySessionStore, SessionStore};

    #[derive(Default)]
    struct FakeBrowserBackend {
        launches: Mutex<Vec<BrowserOpenRequest>>,
    }

    impl BrowserBackend for FakeBrowserBackend {
        fn open(&self, request: BrowserOpenRequest) -> ToolResult<BrowserLaunch> {
            self.launches
                .lock()
                .expect("fake launch lock")
                .push(request);
            Ok(BrowserLaunch {
                handle: Arc::new(FakeBrowserHandle),
                current_url: "https://example.com/app".to_string(),
                title: Some("Example App".to_string()),
            })
        }
    }

    struct FakeBrowserHandle;

    impl BrowserHandle for FakeBrowserHandle {
        fn snapshot(&self, _request: &BrowserSnapshotRequest) -> ToolResult<BrowserSnapshotRecord> {
            Ok(BrowserSnapshotRecord {
                browser_id: BrowserId::from("browser_fake"),
                current_url: "https://example.com/app".to_string(),
                title: Some("Example App".to_string()),
                text_preview: vec![
                    "Dashboard".to_string(),
                    "Queued builds".to_string(),
                    "Recent deploys".to_string(),
                ],
                interactive_elements: vec![
                    BrowserSnapshotElement {
                        kind: BrowserSnapshotElementKind::Button,
                        text: Some("Deploy".to_string()),
                        target: Some("button".to_string()),
                        selector_hint: Some("#deploy".to_string()),
                    },
                    BrowserSnapshotElement {
                        kind: BrowserSnapshotElementKind::Link,
                        text: Some("Settings".to_string()),
                        target: Some("/settings".to_string()),
                        selector_hint: Some("a.settings".to_string()),
                    },
                ],
                html_preview: vec!["<html><body><main>Example</main></body></html>".to_string()],
            })
        }

        fn click(
            &self,
            selector: &str,
            _wait_for_navigation: bool,
        ) -> ToolResult<BrowserInteractionOutcome> {
            Ok(BrowserInteractionOutcome {
                current_url: format!("https://example.com/clicked?selector={selector}"),
                title: Some("Clicked Example App".to_string()),
            })
        }

        fn type_text(&self, request: &BrowserTypeRequest) -> ToolResult<BrowserInteractionOutcome> {
            let mode = if request.clear_first {
                "replace"
            } else {
                "append"
            };
            let submit_suffix = if request.submit { "&submit=1" } else { "" };
            Ok(BrowserInteractionOutcome {
                current_url: format!(
                    "https://example.com/typed?selector={}&mode={mode}{submit_suffix}",
                    request.selector
                ),
                title: Some("Typed Example App".to_string()),
            })
        }

        fn eval(&self, request: &BrowserEvalRequest) -> ToolResult<BrowserEvalOutcome> {
            Ok(BrowserEvalOutcome {
                browser: BrowserInteractionOutcome {
                    current_url: format!(
                        "https://example.com/eval?await={}",
                        request.await_promise
                    ),
                    title: Some("Evaluated Example App".to_string()),
                },
                result: json!({
                    "script_length": request.script.chars().count(),
                    "await_promise": request.await_promise,
                }),
            })
        }

        fn close(&self, _fire_unload: bool) -> ToolResult<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn open_browser_persists_and_publishes_typed_summary() {
        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let events = SessionEventStream::default();
        let backend = Arc::new(FakeBrowserBackend::default());
        let manager = SessionBrowserManager::with_backend(store.clone(), events.clone(), backend);
        let runtime = BrowserRuntimeContext {
            session_id: Some(agent::types::SessionId::from("session-1")),
            agent_session_id: Some(agent::types::AgentSessionId::from("agent-session-1")),
            turn_id: Some(agent::types::TurnId::from("turn-1")),
            parent_agent_id: Some(agent::types::AgentId::from("agent-1")),
            task_id: Some(agent::types::TaskId::from("task-1")),
        };
        let summary = manager
            .open_browser(
                runtime.clone(),
                BrowserOpenRequest {
                    url: "https://example.com".to_string(),
                    headless: false,
                    viewport: Some(agent::types::BrowserViewportRecord {
                        width: 1280,
                        height: 720,
                    }),
                },
            )
            .await
            .expect("browser should open");

        assert_eq!(summary.status, BrowserStatus::Open);
        assert_eq!(summary.current_url, "https://example.com/app");
        assert_eq!(summary.title.as_deref(), Some("Example App"));
        assert!(!summary.headless);
        assert_eq!(
            summary.viewport,
            Some(agent::types::BrowserViewportRecord {
                width: 1280,
                height: 720,
            })
        );

        let published = events.drain();
        assert!(matches!(
            published.as_slice(),
            [SessionEvent::BrowserOpened { summary: published_summary }]
                if published_summary.browser_id == summary.browser_id
        ));

        let persisted = store
            .events(&agent::types::SessionId::from("session-1"))
            .await
            .expect("session events should load");
        assert!(persisted.iter().any(|event| matches!(
            &event.event,
            SessionEventKind::BrowserOpened { summary: persisted_summary }
                if persisted_summary.browser_id == summary.browser_id
        )));
    }

    #[tokio::test]
    async fn snapshot_browser_uses_current_session_browser_when_unambiguous() {
        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let events = SessionEventStream::default();
        let backend = Arc::new(FakeBrowserBackend::default());
        let manager = SessionBrowserManager::with_backend(store, events, backend);
        let runtime = BrowserRuntimeContext {
            session_id: Some(agent::types::SessionId::from("session-1")),
            agent_session_id: Some(agent::types::AgentSessionId::from("agent-session-1")),
            turn_id: Some(agent::types::TurnId::from("turn-1")),
            parent_agent_id: Some(agent::types::AgentId::from("agent-1")),
            task_id: Some(agent::types::TaskId::from("task-1")),
        };
        let summary = manager
            .open_browser(
                runtime.clone(),
                BrowserOpenRequest {
                    url: "https://example.com".to_string(),
                    headless: true,
                    viewport: None,
                },
            )
            .await
            .expect("browser should open");

        let snapshot = manager
            .snapshot_browser(
                runtime,
                BrowserSnapshotRequest {
                    browser_id: None,
                    include_html: true,
                    max_text_lines: 8,
                    max_elements: 4,
                    max_html_chars: 256,
                },
            )
            .await
            .expect("browser snapshot should resolve the only browser");

        assert_eq!(snapshot.browser_id, summary.browser_id);
        assert_eq!(snapshot.current_url, "https://example.com/app");
        assert_eq!(snapshot.title.as_deref(), Some("Example App"));
        assert_eq!(snapshot.text_preview.len(), 3);
        assert_eq!(snapshot.interactive_elements.len(), 2);
        assert_eq!(snapshot.html_preview.len(), 1);
    }

    #[tokio::test]
    async fn snapshot_browser_requires_browser_id_when_multiple_browsers_are_open() {
        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let events = SessionEventStream::default();
        let backend = Arc::new(FakeBrowserBackend::default());
        let manager = SessionBrowserManager::with_backend(store, events, backend);
        let runtime = BrowserRuntimeContext {
            session_id: Some(agent::types::SessionId::from("session-1")),
            agent_session_id: Some(agent::types::AgentSessionId::from("agent-session-1")),
            turn_id: Some(agent::types::TurnId::from("turn-1")),
            parent_agent_id: Some(agent::types::AgentId::from("agent-1")),
            task_id: Some(agent::types::TaskId::from("task-1")),
        };
        for suffix in ["one", "two"] {
            manager
                .open_browser(
                    runtime.clone(),
                    BrowserOpenRequest {
                        url: format!("https://example.com/{suffix}"),
                        headless: true,
                        viewport: None,
                    },
                )
                .await
                .expect("browser should open");
        }

        let error = manager
            .snapshot_browser(
                runtime,
                BrowserSnapshotRequest {
                    browser_id: None,
                    include_html: false,
                    max_text_lines: 8,
                    max_elements: 4,
                    max_html_chars: 256,
                },
            )
            .await
            .expect_err("snapshot should require explicit browser_id");

        assert!(
            error
                .to_string()
                .contains("browser tools require browser_id when multiple browsers are open")
        );
    }

    #[tokio::test]
    async fn click_browser_updates_summary_and_publishes_event() {
        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let events = SessionEventStream::default();
        let backend = Arc::new(FakeBrowserBackend::default());
        let manager = SessionBrowserManager::with_backend(store.clone(), events.clone(), backend);
        let runtime = BrowserRuntimeContext {
            session_id: Some(agent::types::SessionId::from("session-1")),
            agent_session_id: Some(agent::types::AgentSessionId::from("agent-session-1")),
            turn_id: Some(agent::types::TurnId::from("turn-1")),
            parent_agent_id: Some(agent::types::AgentId::from("agent-1")),
            task_id: Some(agent::types::TaskId::from("task-1")),
        };
        let summary = manager
            .open_browser(
                runtime.clone(),
                BrowserOpenRequest {
                    url: "https://example.com".to_string(),
                    headless: true,
                    viewport: None,
                },
            )
            .await
            .expect("browser should open");
        let _ = events.drain();

        let clicked = manager
            .click_browser(
                runtime.clone(),
                BrowserClickRequest {
                    browser_id: Some(summary.browser_id.clone()),
                    selector: "#deploy".to_string(),
                    wait_for_navigation: false,
                },
            )
            .await
            .expect("browser click should update the typed summary");

        assert_eq!(
            clicked.current_url,
            "https://example.com/clicked?selector=#deploy"
        );
        assert_eq!(clicked.title.as_deref(), Some("Clicked Example App"));
        assert!(clicked.updated_at_unix_s.is_some());

        let published = events.drain();
        assert!(matches!(
            published.as_slice(),
            [SessionEvent::BrowserUpdated { summary: published_summary }]
                if published_summary.browser_id == summary.browser_id
                && published_summary.current_url == "https://example.com/clicked?selector=#deploy"
        ));

        let persisted = store
            .events(&agent::types::SessionId::from("session-1"))
            .await
            .expect("session events should load");
        assert!(persisted.iter().any(|event| matches!(
            &event.event,
            SessionEventKind::BrowserUpdated { summary: persisted_summary }
                if persisted_summary.browser_id == summary.browser_id
                && persisted_summary.current_url == "https://example.com/clicked?selector=#deploy"
        )));
    }

    #[tokio::test]
    async fn type_browser_updates_summary_and_publishes_event() {
        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let events = SessionEventStream::default();
        let backend = Arc::new(FakeBrowserBackend::default());
        let manager = SessionBrowserManager::with_backend(store.clone(), events.clone(), backend);
        let runtime = BrowserRuntimeContext {
            session_id: Some(agent::types::SessionId::from("session-1")),
            agent_session_id: Some(agent::types::AgentSessionId::from("agent-session-1")),
            turn_id: Some(agent::types::TurnId::from("turn-1")),
            parent_agent_id: Some(agent::types::AgentId::from("agent-1")),
            task_id: Some(agent::types::TaskId::from("task-1")),
        };
        let summary = manager
            .open_browser(
                runtime.clone(),
                BrowserOpenRequest {
                    url: "https://example.com".to_string(),
                    headless: true,
                    viewport: None,
                },
            )
            .await
            .expect("browser should open");
        let _ = events.drain();

        let typed = manager
            .type_browser(
                runtime.clone(),
                BrowserTypeRequest {
                    browser_id: Some(summary.browser_id.clone()),
                    selector: "#search".to_string(),
                    text: "release notes".to_string(),
                    clear_first: true,
                    submit: true,
                    wait_for_navigation: false,
                },
            )
            .await
            .expect("browser typing should update the typed summary");

        assert_eq!(
            typed.current_url,
            "https://example.com/typed?selector=#search&mode=replace&submit=1"
        );
        assert_eq!(typed.title.as_deref(), Some("Typed Example App"));
        assert!(typed.updated_at_unix_s.is_some());

        let published = events.drain();
        assert!(matches!(
            published.as_slice(),
            [SessionEvent::BrowserUpdated { summary: published_summary }]
                if published_summary.browser_id == summary.browser_id
                && published_summary.current_url
                    == "https://example.com/typed?selector=#search&mode=replace&submit=1"
        ));

        let persisted = store
            .events(&agent::types::SessionId::from("session-1"))
            .await
            .expect("session events should load");
        assert!(persisted.iter().any(|event| matches!(
            &event.event,
            SessionEventKind::BrowserUpdated { summary: persisted_summary }
                if persisted_summary.browser_id == summary.browser_id
                && persisted_summary.current_url
                    == "https://example.com/typed?selector=#search&mode=replace&submit=1"
        )));
    }

    #[tokio::test]
    async fn eval_browser_updates_summary_and_returns_typed_result() {
        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let events = SessionEventStream::default();
        let backend = Arc::new(FakeBrowserBackend::default());
        let manager = SessionBrowserManager::with_backend(store.clone(), events.clone(), backend);
        let runtime = BrowserRuntimeContext {
            session_id: Some(agent::types::SessionId::from("session-1")),
            agent_session_id: Some(agent::types::AgentSessionId::from("agent-session-1")),
            turn_id: Some(agent::types::TurnId::from("turn-1")),
            parent_agent_id: Some(agent::types::AgentId::from("agent-1")),
            task_id: Some(agent::types::TaskId::from("task-1")),
        };
        let summary = manager
            .open_browser(
                runtime.clone(),
                BrowserOpenRequest {
                    url: "https://example.com".to_string(),
                    headless: true,
                    viewport: None,
                },
            )
            .await
            .expect("browser should open");
        let _ = events.drain();

        let (evaluated, result) = manager
            .eval_browser(
                runtime.clone(),
                BrowserEvalRequest {
                    browser_id: Some(summary.browser_id.clone()),
                    script: "document.title".to_string(),
                    await_promise: true,
                },
            )
            .await
            .expect("browser eval should update the typed summary");

        assert_eq!(evaluated.current_url, "https://example.com/eval?await=true");
        assert_eq!(evaluated.title.as_deref(), Some("Evaluated Example App"));
        assert_eq!(
            result,
            json!({
                "script_length": 14,
                "await_promise": true,
            })
        );

        let published = events.drain();
        assert!(matches!(
            published.as_slice(),
            [SessionEvent::BrowserUpdated { summary: published_summary }]
                if published_summary.browser_id == summary.browser_id
                && published_summary.current_url == "https://example.com/eval?await=true"
        ));

        let persisted = store
            .events(&agent::types::SessionId::from("session-1"))
            .await
            .expect("session events should load");
        assert!(persisted.iter().any(|event| matches!(
            &event.event,
            SessionEventKind::BrowserUpdated { summary: persisted_summary }
                if persisted_summary.browser_id == summary.browser_id
                && persisted_summary.current_url == "https://example.com/eval?await=true"
        )));
    }

    #[tokio::test]
    async fn close_browser_updates_summary_and_removes_browser_from_registry() {
        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let events = SessionEventStream::default();
        let backend = Arc::new(FakeBrowserBackend::default());
        let manager = SessionBrowserManager::with_backend(store.clone(), events.clone(), backend);
        let runtime = BrowserRuntimeContext {
            session_id: Some(agent::types::SessionId::from("session-1")),
            agent_session_id: Some(agent::types::AgentSessionId::from("agent-session-1")),
            turn_id: Some(agent::types::TurnId::from("turn-1")),
            parent_agent_id: Some(agent::types::AgentId::from("agent-1")),
            task_id: Some(agent::types::TaskId::from("task-1")),
        };
        let summary = manager
            .open_browser(
                runtime.clone(),
                BrowserOpenRequest {
                    url: "https://example.com".to_string(),
                    headless: true,
                    viewport: None,
                },
            )
            .await
            .expect("browser should open");
        let _ = events.drain();

        let closed = manager
            .close_browser(
                runtime.clone(),
                BrowserCloseRequest {
                    browser_id: Some(summary.browser_id.clone()),
                    fire_unload: true,
                },
            )
            .await
            .expect("browser close should update the typed summary");

        assert_eq!(closed.browser_id, summary.browser_id);
        assert_eq!(closed.status, BrowserStatus::Closed);
        assert!(closed.updated_at_unix_s.is_some());
        assert!(closed.closed_at_unix_s.is_some());

        let published = events.drain();
        assert!(matches!(
            published.as_slice(),
            [SessionEvent::BrowserUpdated { summary: published_summary }]
                if published_summary.browser_id == summary.browser_id
                && published_summary.status == BrowserStatus::Closed
        ));

        let persisted = store
            .events(&agent::types::SessionId::from("session-1"))
            .await
            .expect("session events should load");
        assert!(persisted.iter().any(|event| matches!(
            &event.event,
            SessionEventKind::BrowserUpdated { summary: persisted_summary }
                if persisted_summary.browser_id == summary.browser_id
                && persisted_summary.status == BrowserStatus::Closed
        )));

        let error = manager
            .snapshot_browser(
                runtime,
                BrowserSnapshotRequest {
                    browser_id: Some(summary.browser_id.clone()),
                    include_html: false,
                    max_text_lines: 4,
                    max_elements: 4,
                    max_html_chars: 128,
                },
            )
            .await
            .expect_err("closed browser should not remain addressable");
        assert!(
            error
                .to_string()
                .contains(&format!("browser {} is not open", summary.browser_id))
        );
    }
}
