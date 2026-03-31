use store::{SessionStore, derive_self_improve_signals};
use types::{SelfImproveSignalRecord, SessionEventEnvelope, SessionId};

pub use types::{SelfImproveSignalKind, SignalSeverity, SignalSource};

#[must_use]
pub fn extract_self_improve_signals(
    events: &[SessionEventEnvelope],
) -> Vec<SelfImproveSignalRecord> {
    derive_self_improve_signals(events)
}

pub async fn session_self_improve_signals<S: SessionStore + ?Sized>(
    store: &S,
    session_id: &SessionId,
) -> store::Result<Vec<SelfImproveSignalRecord>> {
    store.self_improve_signals(session_id).await
}

pub async fn all_self_improve_signals<S: SessionStore + ?Sized>(
    store: &S,
) -> store::Result<Vec<SelfImproveSignalRecord>> {
    let sessions = store.list_sessions().await?;
    let mut signals = Vec::new();
    for session in sessions {
        signals.extend(store.self_improve_signals(&session.session_id).await?);
    }
    signals.sort_by(|left, right| {
        right
            .timestamp_ms
            .cmp(&left.timestamp_ms)
            .then_with(|| left.signal_id.as_str().cmp(right.signal_id.as_str()))
    });
    Ok(signals)
}

#[cfg(test)]
mod tests {
    use super::{SelfImproveSignalKind, SignalSeverity, all_self_improve_signals};
    use store::{EventSink, InMemorySessionStore};
    use types::{
        AgentSessionId, HookEffect, HookEvent, HookResult, SessionEventEnvelope, SessionEventKind,
        SessionId,
    };

    #[test]
    fn extracts_store_backed_signal_shapes() {
        let events = vec![
            SessionEventEnvelope::new(
                SessionId::from("session-signals"),
                AgentSessionId::from("agent-signals"),
                Some("turn-signals".into()),
                None,
                SessionEventKind::HookCompleted {
                    hook_name: "stop-hook".to_string(),
                    event: HookEvent::Stop,
                    output: HookResult {
                        effects: vec![HookEffect::Stop {
                            reason: "policy stop".to_string(),
                        }],
                    },
                },
            ),
            SessionEventEnvelope::new(
                SessionId::from("session-signals"),
                AgentSessionId::from("agent-signals"),
                Some("turn-signals".into()),
                None,
                SessionEventKind::Notification {
                    source: "loop_detector".to_string(),
                    message: "loop_detector [critical] repeated tool call".to_string(),
                },
            ),
        ];

        let signals = super::extract_self_improve_signals(&events);

        assert_eq!(signals.len(), 2);
        assert!(
            signals
                .iter()
                .any(|signal| matches!(signal.kind, SelfImproveSignalKind::HookStop))
        );
        assert!(
            signals
                .iter()
                .any(|signal| matches!(signal.kind, SelfImproveSignalKind::LoopDetectorCritical))
        );
    }

    #[tokio::test]
    async fn aggregates_signals_across_sessions() {
        let store = InMemorySessionStore::new();
        store
            .append(SessionEventEnvelope::new(
                SessionId::from("session-store-signals"),
                AgentSessionId::from("agent-store-signals"),
                Some("turn-store-signals".into()),
                None,
                SessionEventKind::TurnFailed {
                    stage: "run_turn_loop".to_string(),
                    error: "backend boom".to_string(),
                },
            ))
            .await
            .unwrap();

        let signals = all_self_improve_signals(&store).await.unwrap();

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].kind, SelfImproveSignalKind::TurnFailed);
        assert_eq!(signals[0].severity, SignalSeverity::Error);
    }
}
