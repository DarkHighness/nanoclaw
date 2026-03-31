use crate::tasks::{SelfImproveTask, SelfImproveTaskKind, SelfImproveTaskPriority};
use std::collections::{BTreeMap, BTreeSet};
use store::SessionStore;
use types::{
    EventId, SelfImproveSignalKind, SelfImproveSignalRecord, SessionId, SignalId, SignalSeverity,
    ToolName, TurnId, new_opaque_id,
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct TaskGroupKey {
    kind: SelfImproveTaskKind,
    session_id: SessionId,
    agent_session_id: types::AgentSessionId,
    turn_id: Option<TurnId>,
    tool_name: Option<ToolName>,
    source_task_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct TaskAccumulator {
    latest_timestamp_ms: u128,
    priority: Option<SelfImproveTaskPriority>,
    source_signal_ids: BTreeSet<SignalId>,
    source_event_ids: BTreeSet<EventId>,
    source_signal_kinds: Vec<SelfImproveSignalKind>,
    details: Vec<String>,
    hook_names: Vec<String>,
    stages: Vec<String>,
}

#[must_use]
pub fn derive_self_improve_tasks(signals: &[SelfImproveSignalRecord]) -> Vec<SelfImproveTask> {
    // Grouping by turn/tool/task keeps one noisy runtime failure from exploding
    // into a pile of near-duplicate improvement tasks. Later phases can decide
    // whether to split or merge further once promotion and corpus semantics land.
    let mut grouped = BTreeMap::<TaskGroupKey, TaskAccumulator>::new();

    for signal in signals {
        let key = TaskGroupKey {
            kind: classify_signal(signal.kind),
            session_id: signal.session_id.clone(),
            agent_session_id: signal.agent_session_id.clone(),
            turn_id: signal.turn_id.clone(),
            tool_name: signal.tool_name.clone(),
            source_task_id: signal.task_id.clone(),
        };
        let entry = grouped.entry(key).or_default();
        entry.latest_timestamp_ms = entry.latest_timestamp_ms.max(signal.timestamp_ms);
        entry.priority = Some(match entry.priority {
            Some(current) => current.max(priority_for_signal(signal.severity)),
            None => priority_for_signal(signal.severity),
        });
        entry.source_signal_ids.insert(signal.signal_id.clone());
        entry
            .source_event_ids
            .extend(signal.event_ids.iter().cloned());
        push_unique_signal_kind(&mut entry.source_signal_kinds, signal.kind);
        push_unique_text(&mut entry.details, signal.summary.clone());
        if let Some(details) = &signal.details {
            push_unique_text(&mut entry.details, details.clone());
        }
        if let Some(stage) = signal
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("stage"))
            .and_then(|value| value.as_str())
        {
            push_unique_text(&mut entry.stages, stage.to_string());
        }
        if let Some(hook_name) = signal
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("hook_name"))
            .and_then(|value| value.as_str())
        {
            push_unique_text(&mut entry.hook_names, hook_name.to_string());
        }
    }

    let mut tasks = grouped
        .into_iter()
        .map(|(key, accumulator)| build_task(key, accumulator))
        .collect::<Vec<_>>();
    tasks.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| left.summary.cmp(&right.summary))
            .then_with(|| left.task_id.cmp(&right.task_id))
    });
    tasks
}

pub async fn session_self_improve_tasks<S: SessionStore + ?Sized>(
    store: &S,
    session_id: &SessionId,
) -> store::Result<Vec<SelfImproveTask>> {
    let signals = store.self_improve_signals(session_id).await?;
    Ok(derive_self_improve_tasks(&signals))
}

pub async fn all_self_improve_tasks<S: SessionStore + ?Sized>(
    store: &S,
) -> store::Result<Vec<SelfImproveTask>> {
    let mut tasks = Vec::new();
    for session in store.list_sessions().await? {
        tasks.extend(session_self_improve_tasks(store, &session.session_id).await?);
    }
    tasks.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| left.summary.cmp(&right.summary))
            .then_with(|| left.task_id.cmp(&right.task_id))
    });
    Ok(tasks)
}

fn build_task(key: TaskGroupKey, accumulator: TaskAccumulator) -> SelfImproveTask {
    let summary = task_summary(&key, &accumulator);
    let objective = task_objective(&key, &accumulator);
    let expected_outcome = task_expected_outcome(key.kind);

    SelfImproveTask {
        task_id: new_opaque_id(),
        kind: key.kind,
        priority: accumulator
            .priority
            .unwrap_or(SelfImproveTaskPriority::Medium),
        summary,
        objective,
        expected_outcome,
        session_id: key.session_id,
        agent_session_id: key.agent_session_id,
        turn_id: key.turn_id,
        source_signal_ids: accumulator.source_signal_ids.into_iter().collect(),
        source_event_ids: accumulator.source_event_ids.into_iter().collect(),
        source_signal_kinds: accumulator.source_signal_kinds,
        relevant_files: relevant_files_for_kind(key.kind),
        tool_name: key.tool_name,
        source_task_id: key.source_task_id,
        details: accumulator.details,
    }
}

fn classify_signal(kind: SelfImproveSignalKind) -> SelfImproveTaskKind {
    match kind {
        SelfImproveSignalKind::HistoryRollback
        | SelfImproveSignalKind::LoopDetectorWarning
        | SelfImproveSignalKind::LoopDetectorCritical => SelfImproveTaskKind::PromptRegressionFix,
        SelfImproveSignalKind::ToolCallFailure | SelfImproveSignalKind::ToolApprovalDenied => {
            SelfImproveTaskKind::ToolSelectionFix
        }
        SelfImproveSignalKind::SubagentFailure | SelfImproveSignalKind::SubagentCancelled => {
            SelfImproveTaskKind::SubagentRoutingFix
        }
        SelfImproveSignalKind::HookStop => SelfImproveTaskKind::HookPolicyFix,
        SelfImproveSignalKind::RetryChurn
        | SelfImproveSignalKind::HighTokenUsage
        | SelfImproveSignalKind::HighTurnLatency => SelfImproveTaskKind::CostLatencyOptimization,
        SelfImproveSignalKind::TurnFailed | SelfImproveSignalKind::TurnStopFailure => {
            SelfImproveTaskKind::RuntimeBugfix
        }
    }
}

fn priority_for_signal(severity: SignalSeverity) -> SelfImproveTaskPriority {
    match severity {
        SignalSeverity::Info => SelfImproveTaskPriority::Low,
        SignalSeverity::Warning => SelfImproveTaskPriority::Medium,
        SignalSeverity::Error => SelfImproveTaskPriority::High,
        SignalSeverity::Critical => SelfImproveTaskPriority::Critical,
    }
}

fn task_summary(key: &TaskGroupKey, accumulator: &TaskAccumulator) -> String {
    match key.kind {
        SelfImproveTaskKind::PromptRegressionFix => {
            if key
                .tool_name
                .as_ref()
                .is_some_and(|tool_name| !tool_name.as_str().is_empty())
            {
                format!(
                    "tighten prompt behavior around repeated `{}` tool usage",
                    key.tool_name.as_ref().expect("checked above")
                )
            } else if accumulator
                .source_signal_kinds
                .contains(&SelfImproveSignalKind::HistoryRollback)
            {
                "fix prompt regression behind manual history rollback".to_string()
            } else {
                "tighten prompt behavior that led to repeated runtime loops".to_string()
            }
        }
        SelfImproveTaskKind::ToolSelectionFix => match &key.tool_name {
            Some(tool_name) => format!("stabilize failing or denied `{tool_name}` tool usage"),
            None => "stabilize failing tool selection decisions".to_string(),
        },
        SelfImproveTaskKind::SubagentRoutingFix => match &key.source_task_id {
            Some(task_id) => format!("stabilize subagent routing for task `{task_id}`"),
            None => "stabilize subagent routing and completion".to_string(),
        },
        SelfImproveTaskKind::HookPolicyFix => {
            if let Some(hook_name) = accumulator.hook_names.first() {
                format!("tighten hook policy for `{hook_name}`")
            } else {
                "tighten hook stop policy".to_string()
            }
        }
        SelfImproveTaskKind::CostLatencyOptimization => {
            "reduce retry, token, or latency churn for this turn shape".to_string()
        }
        SelfImproveTaskKind::RuntimeBugfix => {
            if let Some(stage) = accumulator.stages.first() {
                format!("fix runtime failure path in `{stage}`")
            } else {
                "fix runtime failure path".to_string()
            }
        }
    }
}

fn task_objective(key: &TaskGroupKey, accumulator: &TaskAccumulator) -> String {
    match key.kind {
        SelfImproveTaskKind::PromptRegressionFix => {
            "Update nanoclaw prompting or steering so the same turn pattern no longer needs operator correction or loop intervention.".to_string()
        }
        SelfImproveTaskKind::ToolSelectionFix => match &key.tool_name {
            Some(tool_name) => format!(
                "Reduce failing or denied `{tool_name}` calls by improving tool choice, tool arguments, or fallback behavior."
            ),
            None => "Reduce failing or denied tool calls by improving tool choice, tool arguments, or fallback behavior.".to_string(),
        },
        SelfImproveTaskKind::SubagentRoutingFix => {
            "Improve subagent spawning, routing, or stop conditions so delegated work completes without abnormal termination.".to_string()
        }
        SelfImproveTaskKind::HookPolicyFix => {
            if let Some(hook_name) = accumulator.hook_names.first() {
                format!(
                    "Adjust hook `{hook_name}` so it only stops turns when policy truly requires it."
                )
            } else {
                "Adjust hook policy so legitimate turns are not stopped prematurely.".to_string()
            }
        }
        SelfImproveTaskKind::CostLatencyOptimization => {
            "Reduce avoidable model retries, token spend, or long-running turn latency without regressing task completion.".to_string()
        }
        SelfImproveTaskKind::RuntimeBugfix => {
            if let Some(stage) = accumulator.stages.first() {
                format!(
                    "Fix nanoclaw runtime handling around `{stage}` so the same failure no longer aborts the turn."
                )
            } else {
                "Fix nanoclaw runtime handling so the same failure no longer aborts the turn.".to_string()
            }
        }
    }
}

fn task_expected_outcome(kind: SelfImproveTaskKind) -> String {
    match kind {
        SelfImproveTaskKind::PromptRegressionFix => {
            "Replay of the source turn no longer needs manual rollback and clears loop-oriented verifiers.".to_string()
        }
        SelfImproveTaskKind::ToolSelectionFix => {
            "The source turn replays with fewer failed or denied tool calls and reaches the intended tool flow.".to_string()
        }
        SelfImproveTaskKind::SubagentRoutingFix => {
            "Delegated work completes without failed or cancelled subagent runs for the same task shape.".to_string()
        }
        SelfImproveTaskKind::HookPolicyFix => {
            "The same turn no longer stops at the hook boundary unless policy still intentionally blocks it.".to_string()
        }
        SelfImproveTaskKind::CostLatencyOptimization => {
            "The same turn shape replays with lower retry churn, lower latency, or lower token usage.".to_string()
        }
        SelfImproveTaskKind::RuntimeBugfix => {
            "The same runtime path no longer throws and the turn reaches a normal terminal outcome.".to_string()
        }
    }
}

fn relevant_files_for_kind(kind: SelfImproveTaskKind) -> Vec<String> {
    match kind {
        SelfImproveTaskKind::PromptRegressionFix => vec![
            "apps/code-agent/src/backend/boot_preamble.rs".to_string(),
            "crates/runtime/src/runtime/turn_loop.rs".to_string(),
        ],
        SelfImproveTaskKind::ToolSelectionFix => vec![
            "crates/runtime/src/runtime/tool_flow.rs".to_string(),
            "crates/tools/src".to_string(),
        ],
        SelfImproveTaskKind::SubagentRoutingFix => vec![
            "crates/runtime/src/subagent_impl.rs".to_string(),
            "apps/code-agent/src/backend/session.rs".to_string(),
        ],
        SelfImproveTaskKind::HookPolicyFix => vec![
            "crates/runtime/src/runtime/hook_effects.rs".to_string(),
            "crates/runtime/src/hooks".to_string(),
        ],
        SelfImproveTaskKind::CostLatencyOptimization => vec![
            "crates/runtime/src/runtime/turn_loop.rs".to_string(),
            "crates/runtime/src/runtime/history.rs".to_string(),
        ],
        SelfImproveTaskKind::RuntimeBugfix => vec![
            "crates/runtime/src/runtime.rs".to_string(),
            "crates/runtime/src/runtime".to_string(),
        ],
    }
}

fn push_unique_signal_kind(
    values: &mut Vec<SelfImproveSignalKind>,
    candidate: SelfImproveSignalKind,
) {
    if !values.contains(&candidate) {
        values.push(candidate);
    }
}

fn push_unique_text(values: &mut Vec<String>, candidate: String) {
    if !candidate.is_empty() && !values.iter().any(|value| value == &candidate) {
        values.push(candidate);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SelfImproveTaskKind, SelfImproveTaskPriority, all_self_improve_tasks,
        derive_self_improve_tasks,
    };
    use store::{EventSink, InMemorySessionStore};
    use types::{
        AgentSessionId, SelfImproveSignalKind, SelfImproveSignalRecord, SessionEventEnvelope,
        SessionEventKind, SessionId, SignalSeverity, SignalSource,
    };

    fn signal(
        kind: SelfImproveSignalKind,
        severity: SignalSeverity,
        summary: &str,
    ) -> SelfImproveSignalRecord {
        SelfImproveSignalRecord {
            signal_id: types::SignalId::new(),
            session_id: SessionId::from("session-task"),
            agent_session_id: AgentSessionId::from("agent-session-task"),
            turn_id: Some("turn-task".into()),
            tool_call_id: None,
            timestamp_ms: 10,
            source: SignalSource::Tool,
            kind,
            severity,
            summary: summary.to_string(),
            event_ids: vec![types::EventId::new()],
            tool_name: Some("bash".into()),
            task_id: None,
            details: None,
            metadata: None,
        }
    }

    #[test]
    fn groups_related_tool_signals_into_one_task() {
        let tasks = derive_self_improve_tasks(&[
            signal(
                SelfImproveSignalKind::ToolApprovalDenied,
                SignalSeverity::Warning,
                "tool `bash` approval denied",
            ),
            signal(
                SelfImproveSignalKind::ToolCallFailure,
                SignalSeverity::Error,
                "tool `bash` failed",
            ),
        ]);

        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].kind, SelfImproveTaskKind::ToolSelectionFix);
        assert_eq!(tasks[0].priority, SelfImproveTaskPriority::High);
        assert_eq!(tasks[0].source_signal_ids.len(), 2);
        assert_eq!(
            tasks[0].tool_name.as_ref().map(|value| value.as_str()),
            Some("bash")
        );
    }

    #[test]
    fn maps_hook_and_latency_signals_to_distinct_task_kinds() {
        let tasks = derive_self_improve_tasks(&[
            SelfImproveSignalRecord {
                signal_id: types::SignalId::new(),
                session_id: SessionId::from("session-hook"),
                agent_session_id: AgentSessionId::from("agent-session-hook"),
                turn_id: Some("turn-hook".into()),
                tool_call_id: None,
                timestamp_ms: 20,
                source: SignalSource::Hook,
                kind: SelfImproveSignalKind::HookStop,
                severity: SignalSeverity::Warning,
                summary: "hook `guard` requested stop".to_string(),
                event_ids: vec![types::EventId::new()],
                tool_name: None,
                task_id: None,
                details: Some("policy stop".to_string()),
                metadata: Some(serde_json::json!({ "hook_name": "guard" })),
            },
            SelfImproveSignalRecord {
                signal_id: types::SignalId::new(),
                session_id: SessionId::from("session-latency"),
                agent_session_id: AgentSessionId::from("agent-session-latency"),
                turn_id: Some("turn-latency".into()),
                tool_call_id: None,
                timestamp_ms: 30,
                source: SignalSource::Turn,
                kind: SelfImproveSignalKind::HighTurnLatency,
                severity: SignalSeverity::Warning,
                summary: "turn exceeded latency budget".to_string(),
                event_ids: vec![types::EventId::new()],
                tool_name: None,
                task_id: None,
                details: None,
                metadata: None,
            },
        ]);

        assert_eq!(tasks.len(), 2);
        assert!(
            tasks
                .iter()
                .any(|task| task.kind == SelfImproveTaskKind::HookPolicyFix)
        );
        assert!(
            tasks
                .iter()
                .any(|task| task.kind == SelfImproveTaskKind::CostLatencyOptimization)
        );
    }

    #[tokio::test]
    async fn derives_runtime_bugfix_tasks_from_store_backed_turn_failures() {
        let store = InMemorySessionStore::new();
        store
            .append(SessionEventEnvelope::new(
                SessionId::from("session-runtime-task"),
                AgentSessionId::from("agent-runtime-task"),
                Some("turn-runtime-task".into()),
                None,
                SessionEventKind::TurnFailed {
                    stage: "run_turn_loop".to_string(),
                    error: "backend boom".to_string(),
                },
            ))
            .await
            .unwrap();

        let tasks = all_self_improve_tasks(&store).await.unwrap();

        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].kind, SelfImproveTaskKind::RuntimeBugfix);
        assert_eq!(tasks[0].priority, SelfImproveTaskPriority::High);
        assert!(
            tasks[0]
                .details
                .iter()
                .any(|detail| detail.contains("backend boom"))
        );
    }
}
