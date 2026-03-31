use crate::{
    SelfImproveTask, all_self_improve_tasks, focus_events, focus_transcript, last_user_prompt,
    session_self_improve_tasks,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use store::SessionStore;
use types::{Message, SessionEventEnvelope, SessionId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelfRegressionSplit {
    Train,
    Validation,
    Holdout,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SelfRegressionCase {
    pub case_id: String,
    pub split: SelfRegressionSplit,
    pub task: SelfImproveTask,
    #[serde(default)]
    pub focus_events: Vec<SessionEventEnvelope>,
    #[serde(default)]
    pub focus_transcript: Vec<Message>,
    #[serde(default)]
    pub agent_session_transcript: Vec<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_user_prompt: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SelfRegressionCorpus {
    #[serde(default)]
    pub train: Vec<SelfRegressionCase>,
    #[serde(default)]
    pub validation: Vec<SelfRegressionCase>,
    #[serde(default)]
    pub holdout: Vec<SelfRegressionCase>,
}

impl SelfRegressionCorpus {
    #[must_use]
    pub fn total_cases(&self) -> usize {
        self.train.len() + self.validation.len() + self.holdout.len()
    }
}

pub async fn session_self_regression_corpus<S: SessionStore + ?Sized>(
    store: &S,
    session_id: &SessionId,
) -> store::Result<SelfRegressionCorpus> {
    let tasks = session_self_improve_tasks(store, session_id).await?;
    build_self_regression_corpus(store, &tasks).await
}

pub async fn all_self_regression_corpus<S: SessionStore + ?Sized>(
    store: &S,
) -> store::Result<SelfRegressionCorpus> {
    let tasks = all_self_improve_tasks(store).await?;
    build_self_regression_corpus(store, &tasks).await
}

pub async fn build_self_regression_corpus<S: SessionStore + ?Sized>(
    store: &S,
    tasks: &[SelfImproveTask],
) -> store::Result<SelfRegressionCorpus> {
    let mut corpus = SelfRegressionCorpus::default();
    let mut events_by_session = BTreeMap::<SessionId, Vec<SessionEventEnvelope>>::new();

    for session_id in tasks.iter().map(|task| task.session_id.clone()) {
        events_by_session.entry(session_id).or_insert_with(Vec::new);
    }

    for (session_id, events) in &mut events_by_session {
        *events = store.events(session_id).await?;
    }

    for task in tasks {
        let Some(events) = events_by_session.get(&task.session_id) else {
            continue;
        };
        let case = build_self_regression_case(task.clone(), events);
        match case.split {
            SelfRegressionSplit::Train => corpus.train.push(case),
            SelfRegressionSplit::Validation => corpus.validation.push(case),
            SelfRegressionSplit::Holdout => corpus.holdout.push(case),
        }
    }

    sort_cases(&mut corpus.train);
    sort_cases(&mut corpus.validation);
    sort_cases(&mut corpus.holdout);
    Ok(corpus)
}

#[must_use]
pub fn build_self_regression_case(
    task: SelfImproveTask,
    session_events: &[SessionEventEnvelope],
) -> SelfRegressionCase {
    let focus_events = focus_events(
        session_events,
        &task.agent_session_id,
        task.turn_id.as_ref(),
    );
    let focus_transcript = focus_transcript(
        session_events,
        &task.agent_session_id,
        task.turn_id.as_ref(),
    );
    let agent_session_transcript =
        crate::agent_session_transcript(session_events, &task.agent_session_id);
    let last_user_prompt =
        last_user_prompt(&focus_transcript).or_else(|| last_user_prompt(&agent_session_transcript));

    SelfRegressionCase {
        case_id: types::new_opaque_id(),
        split: split_for_task(&task),
        task,
        focus_events,
        focus_transcript,
        agent_session_transcript,
        last_user_prompt,
    }
}

fn split_for_task(task: &SelfImproveTask) -> SelfRegressionSplit {
    let mut hasher = DefaultHasher::new();
    task.session_id.hash(&mut hasher);
    task.agent_session_id.hash(&mut hasher);
    task.turn_id.hash(&mut hasher);
    task.kind.hash(&mut hasher);
    task.tool_name.hash(&mut hasher);
    task.source_task_id.hash(&mut hasher);
    match hasher.finish() % 10 {
        0 => SelfRegressionSplit::Holdout,
        1 | 2 => SelfRegressionSplit::Validation,
        _ => SelfRegressionSplit::Train,
    }
}

fn sort_cases(cases: &mut [SelfRegressionCase]) {
    cases.sort_by(|left, right| {
        left.task
            .session_id
            .cmp(&right.task.session_id)
            .then_with(|| left.task.turn_id.cmp(&right.task.turn_id))
            .then_with(|| left.task.summary.cmp(&right.task.summary))
    });
}

#[cfg(test)]
mod tests {
    use super::{
        SelfRegressionSplit, all_self_regression_corpus, build_self_regression_case,
        build_self_regression_corpus,
    };
    use crate::{SelfImproveTask, SelfImproveTaskKind, SelfImproveTaskPriority};
    use store::{EventSink, InMemorySessionStore};
    use types::{
        AgentSessionId, Message, SessionEventEnvelope, SessionEventKind, SessionId, SignalId,
        TurnId,
    };

    fn task(session_id: &str, turn_id: &str, summary: &str) -> SelfImproveTask {
        SelfImproveTask {
            task_id: format!("task-{turn_id}"),
            kind: SelfImproveTaskKind::RuntimeBugfix,
            priority: SelfImproveTaskPriority::High,
            summary: summary.to_string(),
            objective: "fix runtime".to_string(),
            expected_outcome: "turn no longer fails".to_string(),
            session_id: SessionId::from(session_id),
            agent_session_id: AgentSessionId::from("agent-corpus"),
            turn_id: Some(TurnId::from(turn_id)),
            source_signal_ids: vec![SignalId::new()],
            source_event_ids: vec![],
            source_signal_kinds: vec![],
            relevant_files: vec!["crates/runtime/src/runtime.rs".to_string()],
            tool_name: None,
            source_task_id: None,
            details: vec![],
        }
    }

    fn event(turn_id: &str, event: SessionEventKind) -> SessionEventEnvelope {
        SessionEventEnvelope::new(
            SessionId::from("session-corpus"),
            AgentSessionId::from("agent-corpus"),
            Some(TurnId::from(turn_id)),
            None,
            event,
        )
    }

    #[test]
    fn builds_case_with_focus_and_agent_session_context() {
        let case = build_self_regression_case(
            task("session-corpus", "turn-2", "fix turn-2"),
            &[
                event(
                    "turn-1",
                    SessionEventKind::TranscriptMessage {
                        message: Message::user("prompt one"),
                    },
                ),
                event(
                    "turn-2",
                    SessionEventKind::TranscriptMessage {
                        message: Message::user("prompt two"),
                    },
                ),
                event(
                    "turn-2",
                    SessionEventKind::TranscriptMessage {
                        message: Message::assistant("response two"),
                    },
                ),
            ],
        );

        assert_eq!(case.focus_events.len(), 2);
        assert_eq!(case.focus_transcript.len(), 2);
        assert_eq!(case.agent_session_transcript.len(), 3);
        assert_eq!(case.last_user_prompt.as_deref(), Some("prompt two"));
    }

    #[tokio::test]
    async fn builds_corpus_and_assigns_all_cases_to_splits() {
        let store = InMemorySessionStore::new();
        store
            .append(event(
                "turn-1",
                SessionEventKind::TranscriptMessage {
                    message: Message::user("prompt one"),
                },
            ))
            .await
            .unwrap();
        store
            .append(event(
                "turn-2",
                SessionEventKind::TranscriptMessage {
                    message: Message::user("prompt two"),
                },
            ))
            .await
            .unwrap();

        let corpus = build_self_regression_corpus(
            &store,
            &[
                task("session-corpus", "turn-1", "fix turn-1"),
                task("session-corpus", "turn-2", "fix turn-2"),
            ],
        )
        .await
        .unwrap();

        assert_eq!(corpus.total_cases(), 2);
    }

    #[tokio::test]
    async fn builds_end_to_end_corpus_from_store_tasks() {
        let store = InMemorySessionStore::new();
        store
            .append(SessionEventEnvelope::new(
                SessionId::from("session-runtime"),
                AgentSessionId::from("agent-runtime"),
                Some(TurnId::from("turn-runtime")),
                None,
                SessionEventKind::TranscriptMessage {
                    message: Message::user("runtime prompt"),
                },
            ))
            .await
            .unwrap();
        store
            .append(SessionEventEnvelope::new(
                SessionId::from("session-runtime"),
                AgentSessionId::from("agent-runtime"),
                Some(TurnId::from("turn-runtime")),
                None,
                SessionEventKind::TurnFailed {
                    stage: "run_turn_loop".to_string(),
                    error: "backend boom".to_string(),
                },
            ))
            .await
            .unwrap();

        let corpus = all_self_regression_corpus(&store).await.unwrap();

        assert_eq!(corpus.total_cases(), 1);
        let case = corpus
            .train
            .iter()
            .chain(corpus.validation.iter())
            .chain(corpus.holdout.iter())
            .next()
            .unwrap();
        assert_eq!(case.task.kind, SelfImproveTaskKind::RuntimeBugfix);
        assert_eq!(case.last_user_prompt.as_deref(), Some("runtime prompt"));
    }

    #[test]
    fn split_assignment_is_stable_for_same_task_shape() {
        let left = build_self_regression_case(
            task("session-corpus", "turn-stable", "stable"),
            &[event(
                "turn-stable",
                SessionEventKind::TranscriptMessage {
                    message: Message::user("stable"),
                },
            )],
        );
        let right = build_self_regression_case(
            task("session-corpus", "turn-stable", "stable"),
            &[event(
                "turn-stable",
                SessionEventKind::TranscriptMessage {
                    message: Message::user("stable"),
                },
            )],
        );

        assert_eq!(left.split, right.split);
        assert!(matches!(
            left.split,
            SelfRegressionSplit::Train
                | SelfRegressionSplit::Validation
                | SelfRegressionSplit::Holdout
        ));
    }
}
