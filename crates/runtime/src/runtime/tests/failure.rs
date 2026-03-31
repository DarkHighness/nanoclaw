use crate::{AgentRuntimeBuilder, ModelBackend, Result};
use async_trait::async_trait;
use futures::stream::BoxStream;
use std::sync::Arc;
use store::{InMemorySessionStore, SessionStore};
use types::{AgentCoreError, ModelEvent, ModelRequest, SessionEventKind};

struct FailingBackend;

#[async_trait]
impl ModelBackend for FailingBackend {
    async fn stream_turn(
        &self,
        _request: ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
        Err(AgentCoreError::ModelBackend("backend boom".to_string()).into())
    }
}

#[tokio::test]
async fn runtime_persists_turn_failure_events() {
    let store = Arc::new(InMemorySessionStore::new());
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(FailingBackend), store.clone()).build();

    let error = runtime.run_user_prompt("hello").await.unwrap_err();
    assert!(error.to_string().contains("backend boom"));

    let events = store.events(&runtime.session_id()).await.unwrap();
    assert!(events.iter().any(|event| matches!(
        &event.event,
        SessionEventKind::TurnFailed { stage, error }
            if stage == "run_turn_loop" && error.contains("backend boom")
    )));
}
