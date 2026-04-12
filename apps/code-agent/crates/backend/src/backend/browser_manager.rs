use crate::backend::SessionEventStream;
use crate::ui::SessionEvent;
use agent::tools::{
    BrowserManager, BrowserOpenRequest, BrowserRuntimeContext, Result as ToolResult, ToolError,
};
use agent::types::{
    BrowserId, BrowserStatus, BrowserSummaryRecord, SessionEventEnvelope, SessionEventKind,
    SessionId, new_opaque_id,
};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use store::SessionStore;

trait BrowserHandle: Send + Sync {}

struct BrowserLaunch {
    handle: Arc<dyn BrowserHandle>,
    current_url: String,
    title: Option<String>,
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

    fn insert_browser(&self, state: Arc<SessionBrowser>) {
        self.browsers
            .lock()
            .expect("browser registry lock")
            .insert(state.summary().browser_id.clone(), state);
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

impl BrowserHandle for HeadlessChromeBrowserHandle {}

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

    impl BrowserHandle for FakeBrowserHandle {}

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
}
