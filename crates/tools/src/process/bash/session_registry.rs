use super::{BashSession, BashSessionId, MAX_TRACKED_BASH_SESSIONS};
use dashmap::DashMap;
use std::sync::{Arc, OnceLock};

// Background bash sessions are inserted by `start` and read by independent
// `poll`/`cancel` calls. A sharded registry keeps those lookups off one global
// lock while preserving the existing completion-based pruning behavior.
type SessionRegistry = DashMap<BashSessionId, Arc<BashSession>>;

static BASH_SESSIONS: OnceLock<SessionRegistry> = OnceLock::new();

pub(super) fn get_session(agent_session_id: &BashSessionId) -> Option<Arc<BashSession>> {
    bash_sessions()
        .get(agent_session_id)
        .map(|entry| Arc::clone(entry.value()))
}

pub(super) fn insert_session(session: Arc<BashSession>) {
    let registry = bash_sessions();
    prune_completed_sessions(registry);
    registry.insert(session.id.clone(), session);
}

fn bash_sessions() -> &'static SessionRegistry {
    BASH_SESSIONS.get_or_init(SessionRegistry::new)
}

fn prune_completed_sessions(registry: &SessionRegistry) {
    if registry.len() < MAX_TRACKED_BASH_SESSIONS {
        return;
    }

    let mut completed = registry
        .iter()
        .filter_map(|entry| {
            entry
                .value()
                .completed_timestamp()
                .map(|finished_at| (entry.key().clone(), finished_at))
        })
        .collect::<Vec<_>>();
    completed.sort_by_key(|(_, finished_at)| *finished_at);

    let remove_count = registry.len().saturating_sub(MAX_TRACKED_BASH_SESSIONS) + 1;
    for (agent_session_id, _) in completed.into_iter().take(remove_count) {
        registry.remove(&agent_session_id);
    }
}
