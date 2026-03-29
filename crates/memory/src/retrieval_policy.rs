use crate::{
    MemoryDocumentMetadata, MemoryListRequest, MemoryScope, MemorySearchRequest, MemoryStatus,
};
use time::{Date, Month, OffsetDateTime};
use types::{AgentSessionId, RunId};

const DAILY_LOG_HALF_LIFE_DAYS: f64 = 30.0;
const RUNTIME_EPISODIC_HALF_LIFE_DAYS: f64 = 14.0;
const WORKING_HALF_LIFE_HOURS: f64 = 12.0;
const COORDINATION_HALF_LIFE_DAYS: f64 = 3.0;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MemoryRetrievalSignals {
    pub(crate) scope_weight: f64,
    pub(crate) recency_multiplier: f64,
    pub(crate) run_match_bonus: f64,
    pub(crate) agent_session_match_bonus: f64,
    pub(crate) agent_match_bonus: f64,
    pub(crate) task_match_bonus: f64,
    pub(crate) stale_penalty: f64,
}

impl MemoryRetrievalSignals {
    #[must_use]
    pub(crate) fn total_multiplier(&self) -> f64 {
        let context_bonus = 1.0
            + self.run_match_bonus
            + self.agent_session_match_bonus
            + self.agent_match_bonus
            + self.task_match_bonus;
        self.scope_weight * self.recency_multiplier * self.stale_penalty * context_bonus
    }
}

pub(crate) fn matches_search_filters(
    path: &str,
    metadata: &MemoryDocumentMetadata,
    request: &MemorySearchRequest,
) -> bool {
    matches_filters(
        path,
        metadata,
        request.path_prefix.as_deref(),
        request.scopes.as_deref(),
        request.tags.as_deref(),
        request.run_id.as_ref(),
        request.agent_session_id.as_ref(),
        request.agent_name.as_deref(),
        request.task_id.as_deref(),
        request.include_stale.unwrap_or(false),
    )
}

pub(crate) fn matches_list_filters(
    path: &str,
    metadata: &MemoryDocumentMetadata,
    request: &MemoryListRequest,
) -> bool {
    matches_filters(
        path,
        metadata,
        request.path_prefix.as_deref(),
        request.scopes.as_deref(),
        request.tags.as_deref(),
        request.run_id.as_ref(),
        request.agent_session_id.as_ref(),
        request.agent_name.as_deref(),
        request.task_id.as_deref(),
        request.include_stale.unwrap_or(false),
    )
}

pub(crate) fn search_signals(
    path: &str,
    metadata: &MemoryDocumentMetadata,
    request: &MemorySearchRequest,
) -> MemoryRetrievalSignals {
    search_signals_on(path, metadata, request, OffsetDateTime::now_utc())
}

fn matches_filters(
    path: &str,
    metadata: &MemoryDocumentMetadata,
    path_prefix: Option<&str>,
    scopes: Option<&[MemoryScope]>,
    tags: Option<&[String]>,
    run_id: Option<&RunId>,
    agent_session_id: Option<&AgentSessionId>,
    agent_name: Option<&str>,
    task_id: Option<&str>,
    include_stale: bool,
) -> bool {
    if path_prefix
        .map(str::trim)
        .filter(|prefix| !prefix.is_empty())
        .is_some_and(|prefix| !path.starts_with(prefix))
    {
        return false;
    }

    if let Some(scopes) = scopes
        && !scopes.is_empty()
        && !scopes.contains(&metadata.scope)
    {
        return false;
    }

    if let Some(tags) = tags
        && !tags.is_empty()
        && !tags
            .iter()
            .all(|tag| metadata.tags.iter().any(|candidate| tag_eq(candidate, tag)))
    {
        return false;
    }

    if let Some(run_id) = run_id
        && metadata.run_id.as_ref() != Some(run_id)
    {
        return false;
    }

    if let Some(agent_session_id) = agent_session_id
        && metadata.agent_session_id.as_ref() != Some(agent_session_id)
    {
        return false;
    }

    if let Some(agent_name) = agent_name
        && metadata
            .agent_name
            .as_deref()
            .is_none_or(|candidate| !candidate.eq_ignore_ascii_case(agent_name))
    {
        return false;
    }

    if let Some(task_id) = task_id
        && metadata.task_id.as_deref() != Some(task_id)
    {
        return false;
    }

    if !status_visible_in_retrieval(metadata.status, include_stale) {
        return false;
    }

    true
}

fn status_visible_in_retrieval(status: MemoryStatus, include_stale: bool) -> bool {
    include_stale || status == MemoryStatus::Ready
}

fn tag_eq(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

fn search_signals_on(
    path: &str,
    metadata: &MemoryDocumentMetadata,
    request: &MemorySearchRequest,
    now: OffsetDateTime,
) -> MemoryRetrievalSignals {
    MemoryRetrievalSignals {
        scope_weight: scope_weight(metadata.scope),
        recency_multiplier: recency_multiplier(path, metadata, now),
        run_match_bonus: match_bonus(metadata.run_id.as_ref(), request.run_id.as_ref(), 0.10),
        agent_session_match_bonus: match_bonus(
            metadata.agent_session_id.as_ref(),
            request.agent_session_id.as_ref(),
            agent_session_bonus(metadata.scope),
        ),
        agent_match_bonus: string_match_bonus(
            metadata.agent_name.as_deref(),
            request.agent_name.as_deref(),
            agent_bonus(metadata.scope),
        ),
        task_match_bonus: string_match_bonus(
            metadata.task_id.as_deref(),
            request.task_id.as_deref(),
            task_bonus(metadata.scope),
        ),
        stale_penalty: stale_penalty(metadata.status),
    }
}

fn scope_weight(scope: MemoryScope) -> f64 {
    match scope {
        MemoryScope::Procedural => 1.30,
        MemoryScope::Semantic => 1.00,
        MemoryScope::Episodic => 0.92,
        MemoryScope::Working => 0.70,
        MemoryScope::Coordination => 0.88,
    }
}

fn agent_session_bonus(scope: MemoryScope) -> f64 {
    match scope {
        MemoryScope::Working => 0.75,
        MemoryScope::Coordination => 0.25,
        MemoryScope::Episodic => 0.20,
        MemoryScope::Procedural | MemoryScope::Semantic => 0.05,
    }
}

fn agent_bonus(scope: MemoryScope) -> f64 {
    match scope {
        MemoryScope::Coordination => 0.35,
        MemoryScope::Episodic => 0.25,
        MemoryScope::Working => 0.10,
        MemoryScope::Procedural | MemoryScope::Semantic => 0.05,
    }
}

fn task_bonus(scope: MemoryScope) -> f64 {
    match scope {
        MemoryScope::Working => 0.65,
        MemoryScope::Coordination => 0.45,
        MemoryScope::Episodic => 0.20,
        MemoryScope::Procedural | MemoryScope::Semantic => 0.05,
    }
}

fn stale_penalty(status: MemoryStatus) -> f64 {
    match status {
        MemoryStatus::Ready => 1.0,
        MemoryStatus::Stale => 0.70,
        MemoryStatus::Superseded => 0.20,
        MemoryStatus::Archived => 0.05,
    }
}

fn match_bonus<T: PartialEq>(candidate: Option<&T>, requested: Option<&T>, bonus: f64) -> f64 {
    match (candidate, requested) {
        (Some(candidate), Some(requested)) if candidate == requested => bonus,
        _ => 0.0,
    }
}

fn string_match_bonus(candidate: Option<&str>, requested: Option<&str>, bonus: f64) -> f64 {
    match (candidate, requested) {
        (Some(candidate), Some(requested)) if candidate.eq_ignore_ascii_case(requested) => bonus,
        _ => 0.0,
    }
}

fn recency_multiplier(path: &str, metadata: &MemoryDocumentMetadata, now: OffsetDateTime) -> f64 {
    match metadata.scope {
        MemoryScope::Procedural | MemoryScope::Semantic => {
            if metadata.layer == "daily-log" {
                daily_log_multiplier(path, now.date())
            } else {
                1.0
            }
        }
        MemoryScope::Episodic => {
            if metadata.layer == "daily-log" {
                daily_log_multiplier(path, now.date())
            } else {
                timestamp_half_life(metadata.updated_at_ms, now, RUNTIME_EPISODIC_HALF_LIFE_DAYS)
                    .max(0.55)
            }
        }
        MemoryScope::Working => {
            timestamp_half_life_hours(metadata.updated_at_ms, now, WORKING_HALF_LIFE_HOURS)
                .max(0.25)
        }
        MemoryScope::Coordination => {
            timestamp_half_life(metadata.updated_at_ms, now, COORDINATION_HALF_LIFE_DAYS).max(0.40)
        }
    }
}

fn daily_log_multiplier(path: &str, today: Date) -> f64 {
    let Some(document_date) = parse_daily_log_date(path) else {
        return 1.0;
    };
    let age_days = (today - document_date).whole_days().max(0) as f64;
    2f64.powf(-(age_days / DAILY_LOG_HALF_LIFE_DAYS))
}

fn timestamp_half_life(
    updated_at_ms: Option<u64>,
    now: OffsetDateTime,
    half_life_days: f64,
) -> f64 {
    let Some(updated_at_ms) = updated_at_ms else {
        return 1.0;
    };
    let age_ms = now.unix_timestamp_nanos() / 1_000_000 - i128::from(updated_at_ms);
    if age_ms <= 0 {
        return 1.0;
    }
    let age_days = age_ms as f64 / 86_400_000.0;
    2f64.powf(-(age_days / half_life_days))
}

fn timestamp_half_life_hours(
    updated_at_ms: Option<u64>,
    now: OffsetDateTime,
    half_life_hours: f64,
) -> f64 {
    let Some(updated_at_ms) = updated_at_ms else {
        return 1.0;
    };
    let age_ms = now.unix_timestamp_nanos() / 1_000_000 - i128::from(updated_at_ms);
    if age_ms <= 0 {
        return 1.0;
    }
    let age_hours = age_ms as f64 / 3_600_000.0;
    2f64.powf(-(age_hours / half_life_hours))
}

fn parse_daily_log_date(path: &str) -> Option<Date> {
    let name = path.rsplit('/').next()?.strip_suffix(".md")?;
    let mut parts = name.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u8>().ok()?;
    let day = parts.next()?.parse::<u8>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Date::from_calendar_date(year, Month::try_from(month).ok()?, day).ok()
}

#[cfg(test)]
mod tests {
    use super::{
        MemoryRetrievalSignals, matches_list_filters, matches_search_filters, search_signals_on,
    };
    use crate::{
        MemoryDocumentMetadata, MemoryListRequest, MemoryScope, MemorySearchRequest, MemoryStatus,
    };
    use time::{Date, Month, PrimitiveDateTime, Time};
    use types::{AgentSessionId, RunId};

    fn ts_ms(year: i32, month: Month, day: u8, hour: u8) -> u64 {
        PrimitiveDateTime::new(
            Date::from_calendar_date(year, month, day).unwrap(),
            Time::from_hms(hour, 0, 0).unwrap(),
        )
        .assume_utc()
        .unix_timestamp_nanos()
        .div_euclid(1_000_000)
        .try_into()
        .unwrap()
    }

    #[test]
    fn search_filters_by_scope_and_runtime_ids() {
        let metadata = MemoryDocumentMetadata {
            scope: MemoryScope::Working,
            layer: "working-agent-session".to_string(),
            run_id: Some(RunId::from("run_1")),
            agent_session_id: Some(AgentSessionId::from("session_1")),
            agent_name: Some("planner".to_string()),
            task_id: Some("task-1".to_string()),
            updated_at_ms: Some(ts_ms(2026, Month::March, 28, 9)),
            promoted_from: None,
            supersedes: Vec::new(),
            tags: vec!["debug".to_string(), "deploy".to_string()],
            status: MemoryStatus::Ready,
        };
        let request = MemorySearchRequest {
            query: "deploy".to_string(),
            limit: None,
            path_prefix: Some(".nanoclaw/memory/working".to_string()),
            scopes: Some(vec![MemoryScope::Working]),
            tags: Some(vec!["deploy".to_string()]),
            run_id: Some(RunId::from("run_1")),
            agent_session_id: Some(AgentSessionId::from("session_1")),
            agent_name: Some("planner".to_string()),
            task_id: Some("task-1".to_string()),
            include_stale: Some(false),
        };

        assert!(matches_search_filters(
            ".nanoclaw/memory/working/sessions/session_1.md",
            &metadata,
            &request
        ));
        assert!(!matches_search_filters(
            ".nanoclaw/memory/working/sessions/session_1.md",
            &MemoryDocumentMetadata {
                status: MemoryStatus::Superseded,
                ..metadata.clone()
            },
            &request
        ));
        assert!(matches_search_filters(
            ".nanoclaw/memory/working/sessions/session_1.md",
            &MemoryDocumentMetadata {
                status: MemoryStatus::Archived,
                ..metadata.clone()
            },
            &MemorySearchRequest {
                include_stale: Some(true),
                ..request.clone()
            }
        ));
    }

    #[test]
    fn list_filters_hide_non_ready_entries_unless_include_stale_is_enabled() {
        let metadata = MemoryDocumentMetadata {
            scope: MemoryScope::Semantic,
            layer: "rule".to_string(),
            status: MemoryStatus::Archived,
            ..MemoryDocumentMetadata::default()
        };
        let base = MemoryListRequest::default();

        assert!(!matches_list_filters(
            ".nanoclaw/memory/semantic/canary.md",
            &metadata,
            &base
        ));
        assert!(matches_list_filters(
            ".nanoclaw/memory/semantic/canary.md",
            &metadata,
            &MemoryListRequest {
                include_stale: Some(true),
                ..base
            }
        ));
    }

    #[test]
    fn working_session_match_gets_priority_bonus() {
        let metadata = MemoryDocumentMetadata {
            scope: MemoryScope::Working,
            layer: "working-agent-session".to_string(),
            agent_session_id: Some(AgentSessionId::from("session_1")),
            updated_at_ms: Some(ts_ms(2026, Month::March, 28, 9)),
            ..MemoryDocumentMetadata::default()
        };
        let request = MemorySearchRequest {
            query: "fix failing test".to_string(),
            limit: None,
            path_prefix: None,
            scopes: None,
            tags: None,
            run_id: None,
            agent_session_id: Some(AgentSessionId::from("session_1")),
            agent_name: None,
            task_id: None,
            include_stale: Some(true),
        };
        let now = PrimitiveDateTime::new(
            Date::from_calendar_date(2026, Month::March, 28).unwrap(),
            Time::from_hms(10, 0, 0).unwrap(),
        )
        .assume_utc();

        assert_eq!(
            search_signals_on(
                ".nanoclaw/memory/working/sessions/session_1.md",
                &metadata,
                &request,
                now
            ),
            MemoryRetrievalSignals {
                scope_weight: 0.70,
                recency_multiplier: 0.9438743126816935,
                run_match_bonus: 0.0,
                agent_session_match_bonus: 0.75,
                agent_match_bonus: 0.0,
                task_match_bonus: 0.0,
                stale_penalty: 1.0,
            }
        );
    }

    #[test]
    fn daily_logs_decay_but_remain_searchable() {
        let metadata = MemoryDocumentMetadata {
            scope: MemoryScope::Episodic,
            layer: "daily-log".to_string(),
            ..MemoryDocumentMetadata::default()
        };
        let request = MemorySearchRequest {
            query: "deploy".to_string(),
            limit: None,
            path_prefix: None,
            scopes: None,
            tags: None,
            run_id: None,
            agent_session_id: None,
            agent_name: None,
            task_id: None,
            include_stale: Some(true),
        };
        let now = PrimitiveDateTime::new(
            Date::from_calendar_date(2026, Month::March, 28).unwrap(),
            Time::MIDNIGHT,
        )
        .assume_utc();
        let signals = search_signals_on("memory/2026-03-21.md", &metadata, &request, now);

        assert_eq!(signals.scope_weight, 0.92);
        assert!(signals.recency_multiplier < 1.0);
        assert!(signals.recency_multiplier > 0.8);
    }
}
