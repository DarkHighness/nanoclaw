use agent::memory::{MemoryBackend, MemoryScope, MemorySearchHit, MemorySearchRequest};
use agent::runtime::{AugmentedUserMessage, UserMessageAugmentationContext, UserMessageAugmentor};
use agent::types::{Message, MessagePart, MessageRole};
use async_trait::async_trait;
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{debug, warn};

const WORKSPACE_MEMORY_RECALL_KIND: &str = "workspace_memory_recall";
const WORKSPACE_MEMORY_RECALL_NAME: &str = "Relevant workspace memory (verify before relying)";
const WORKSPACE_MEMORY_RECALL_METADATA_KEY: &str = "workspace_memory_recall";
const RECALL_TIMEOUT_MS: u64 = 200;
const RECALL_LIMIT: usize = 3;
const RECALL_CANDIDATE_LIMIT: usize = 6;
const RECALL_WORKING_CANDIDATE_LIMIT: usize = 3;
const RECALL_MAX_SNIPPET_CHARS: usize = 220;
const RECALL_MAX_BLOCK_CHARS: usize = 1_400;
const RECALL_QUERY_TERM_LIMIT: usize = 5;
const RECALL_QUERY_FALLBACK_TERM_LIMIT: usize = 3;

#[derive(Clone)]
pub(crate) struct WorkspaceMemoryRecallAugmentor {
    backend: Arc<dyn MemoryBackend>,
    timeout_ms: u64,
}

impl WorkspaceMemoryRecallAugmentor {
    pub(crate) fn new(backend: Arc<dyn MemoryBackend>) -> Self {
        Self {
            backend,
            timeout_ms: RECALL_TIMEOUT_MS,
        }
    }

    fn build_prefix_message(
        &self,
        context: &UserMessageAugmentationContext,
        backend_name: &str,
        block: String,
        hit_count: usize,
    ) -> Message {
        // Keep recall as a distinct synthetic message instead of mutating the
        // operator's prompt text in place. The runtime should see
        // "recalled memory" and "original user request" as separate turns.
        let mut message = Message::new(
            MessageRole::User,
            vec![MessagePart::reference(
                WORKSPACE_MEMORY_RECALL_KIND,
                Some(WORKSPACE_MEMORY_RECALL_NAME.to_string()),
                None,
                Some(block),
            )],
        );
        message.metadata.insert(
            WORKSPACE_MEMORY_RECALL_METADATA_KEY.to_string(),
            json!({
                "backend": backend_name,
                "hits": hit_count,
                "session_id": context.session_id,
                "agent_session_id": context.agent_session_id,
            }),
        );
        message
    }
}

#[async_trait]
impl UserMessageAugmentor for WorkspaceMemoryRecallAugmentor {
    async fn augment_user_message(
        &self,
        context: &UserMessageAugmentationContext,
        message: Message,
    ) -> agent::runtime::Result<AugmentedUserMessage> {
        if !matches!(message.role, MessageRole::User)
            || message
                .metadata
                .contains_key(WORKSPACE_MEMORY_RECALL_METADATA_KEY)
        {
            return Ok(AugmentedUserMessage::unchanged(message));
        }

        let query = message.text_content();
        if !is_recall_candidate(&query) {
            return Ok(AugmentedUserMessage::unchanged(message));
        }

        let Some((backend_name, hits)) = self.search_recall_hits(context, &query).await else {
            return Ok(AugmentedUserMessage::unchanged(message));
        };
        let Some(block) = format_recall_block(&hits) else {
            return Ok(AugmentedUserMessage::unchanged(message));
        };

        Ok(AugmentedUserMessage {
            prefix_messages: vec![self.build_prefix_message(
                context,
                &backend_name,
                block,
                hits.len(),
            )],
            message,
        })
    }
}

impl WorkspaceMemoryRecallAugmentor {
    async fn search_recall_hits(
        &self,
        context: &UserMessageAugmentationContext,
        query: &str,
    ) -> Option<(String, Vec<MemorySearchHit>)> {
        let started_at = std::time::Instant::now();
        let mut backend_name = None;
        let mut collected = Vec::new();

        // Query the active session's working memory first so post-compaction
        // continuation beats older durable notes when both match.
        if let Some((name, working_hits)) = self
            .search_hits(
                query,
                vec![MemoryScope::Working],
                Some(context.session_id.clone()),
                RECALL_WORKING_CANDIDATE_LIMIT,
                started_at,
            )
            .await
        {
            backend_name = Some(name);
            merge_recall_hits(&mut collected, working_hits);
        }
        if collected.len() < RECALL_LIMIT {
            if let Some((name, durable_hits)) = self
                .search_hits(
                    query,
                    vec![MemoryScope::Procedural, MemoryScope::Semantic],
                    None,
                    RECALL_CANDIDATE_LIMIT,
                    started_at,
                )
                .await
            {
                backend_name.get_or_insert(name);
                merge_recall_hits(&mut collected, durable_hits);
            }
        }
        (!collected.is_empty()).then(|| (backend_name.unwrap_or_default(), collected))
    }

    async fn search_hits(
        &self,
        query: &str,
        scopes: Vec<MemoryScope>,
        session_id: Option<agent::types::SessionId>,
        candidate_limit: usize,
        started_at: std::time::Instant,
    ) -> Option<(String, Vec<MemorySearchHit>)> {
        for search_query in build_search_queries(query) {
            let remaining =
                Duration::from_millis(self.timeout_ms).saturating_sub(started_at.elapsed());
            if remaining.is_zero() {
                debug!("workspace memory recall timed out before search completed");
                return None;
            }

            let search = MemorySearchRequest {
                query: search_query,
                limit: Some(candidate_limit),
                path_prefix: None,
                scopes: Some(scopes.clone()),
                types: None,
                tags: None,
                session_id: session_id.clone(),
                agent_session_id: None,
                agent_name: None,
                task_id: None,
                include_stale: Some(false),
            };

            let response = match timeout(remaining, self.backend.search(search)).await {
                Ok(Ok(response)) => response,
                Ok(Err(error)) => {
                    warn!(error = %error, "workspace memory recall search failed");
                    return None;
                }
                Err(_) => {
                    debug!("workspace memory recall timed out");
                    return None;
                }
            };
            let hits = select_recall_hits(&response.hits);
            if !hits.is_empty() {
                return Some((response.backend, hits));
            }
        }
        None
    }
}

fn merge_recall_hits(existing: &mut Vec<MemorySearchHit>, incoming: Vec<MemorySearchHit>) {
    let mut seen_paths = existing
        .iter()
        .map(|hit| hit.path.clone())
        .collect::<BTreeSet<_>>();
    for hit in incoming {
        if !seen_paths.insert(hit.path.clone()) {
            continue;
        }
        existing.push(hit);
        if existing.len() == RECALL_LIMIT {
            break;
        }
    }
}

fn is_recall_candidate(query: &str) -> bool {
    query.split_whitespace().take(2).count() >= 2
}

fn build_search_queries(query: &str) -> Vec<String> {
    let raw_terms = tokenize_query(query);
    if raw_terms.len() < 2 {
        return Vec::new();
    }

    let keyword_terms = raw_terms
        .iter()
        .filter(|term| !is_query_stop_word(term))
        .cloned()
        .collect::<Vec<_>>();
    let primary_terms = if keyword_terms.len() >= 2 {
        keyword_terms
    } else {
        raw_terms
    };

    let mut queries = Vec::new();
    push_query(
        &mut queries,
        primary_terms
            .iter()
            .take(RECALL_QUERY_TERM_LIMIT)
            .cloned()
            .collect(),
    );
    push_query(
        &mut queries,
        informative_terms(&primary_terms, RECALL_QUERY_FALLBACK_TERM_LIMIT),
    );
    queries
}

fn tokenize_query(query: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    query
        .split(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(|token| token.to_ascii_lowercase())
        .filter(|token| token.chars().count() >= 2 || token.chars().any(|ch| ch.is_ascii_digit()))
        .filter(|token| seen.insert(token.clone()))
        .collect()
}

fn informative_terms(terms: &[String], limit: usize) -> Vec<String> {
    let mut ranked = terms
        .iter()
        .enumerate()
        .map(|(index, term)| (index, term.chars().count(), term.clone()))
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    let selected = ranked
        .into_iter()
        .take(limit)
        .map(|(index, _, term)| (index, term))
        .collect::<Vec<_>>();
    let mut in_order = selected;
    in_order.sort_by_key(|(index, _)| *index);
    in_order.into_iter().map(|(_, term)| term).collect()
}

fn push_query(queries: &mut Vec<String>, terms: Vec<String>) {
    if terms.len() < 2 {
        return;
    }
    let query = terms.join(" ");
    if !query.is_empty() && !queries.iter().any(|existing| existing == &query) {
        queries.push(query);
    }
}

fn is_query_stop_word(term: &str) -> bool {
    matches!(
        term,
        "a" | "an"
            | "and"
            | "are"
            | "be"
            | "can"
            | "could"
            | "did"
            | "do"
            | "does"
            | "for"
            | "from"
            | "help"
            | "how"
            | "i"
            | "if"
            | "in"
            | "is"
            | "it"
            | "me"
            | "my"
            | "of"
            | "on"
            | "or"
            | "please"
            | "should"
            | "that"
            | "the"
            | "this"
            | "to"
            | "use"
            | "using"
            | "we"
            | "what"
            | "when"
            | "where"
            | "which"
            | "who"
            | "why"
            | "with"
            | "would"
            | "you"
            | "your"
    )
}

fn select_recall_hits(hits: &[MemorySearchHit]) -> Vec<MemorySearchHit> {
    let preferred = hits
        .iter()
        .filter(|hit| hit.document_metadata.layer != "auto-memory-index")
        .collect::<Vec<_>>();
    let source = if preferred.is_empty() {
        hits.iter().collect::<Vec<_>>()
    } else {
        preferred
    };

    let mut seen_paths = BTreeSet::new();
    let mut selected = Vec::new();
    for hit in source {
        let snippet = normalize_snippet(&hit.snippet);
        if snippet.is_empty() || !seen_paths.insert(hit.path.clone()) {
            continue;
        }
        selected.push(hit.clone());
        if selected.len() == RECALL_LIMIT {
            break;
        }
    }
    selected
}

fn format_recall_block(hits: &[MemorySearchHit]) -> Option<String> {
    if hits.is_empty() {
        return None;
    }

    let mut lines = vec![
        "Use these notes only if they are relevant. Verify against the current workspace before acting."
            .to_string(),
    ];
    let mut total_chars = lines[0].len();
    for hit in hits {
        let snippet = normalize_snippet(&hit.snippet);
        if snippet.is_empty() {
            continue;
        }
        let scope = hit.document_metadata.scope.as_str();
        let kind = hit
            .document_metadata
            .memory_type
            .map(|memory_type| format!("/{memory_type}", memory_type = memory_type.as_str()))
            .unwrap_or_default();
        let line = format!("- {} [{}{}]: {}", hit.path, scope, kind, snippet);
        if total_chars + line.len() > RECALL_MAX_BLOCK_CHARS {
            lines
                .push("- Additional relevant memories were omitted to keep recall concise.".into());
            break;
        }
        total_chars += line.len();
        lines.push(line);
    }
    lines.push("Ignore this recall message if it does not help with the current request.".into());
    Some(lines.join("\n"))
}

fn normalize_snippet(snippet: &str) -> String {
    let collapsed = snippet.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(collapsed.trim(), RECALL_MAX_SNIPPET_CHARS)
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated = text.chars().take(max_chars).collect::<String>();
    format!("{}...", truncated.trim_end())
}

#[cfg(test)]
mod tests {
    use super::{
        WORKSPACE_MEMORY_RECALL_METADATA_KEY, WorkspaceMemoryRecallAugmentor, build_search_queries,
    };
    use agent::memory::{MemoryBackend, MemoryCoreBackend, MemoryRecordRequest, MemoryScope};
    use agent::runtime::{UserMessageAugmentationContext, UserMessageAugmentor};
    use agent::types::{AgentSessionId, Message, MessageRole, SessionId};
    use std::sync::Arc;

    fn context() -> UserMessageAugmentationContext {
        UserMessageAugmentationContext {
            session_id: SessionId::from("session-test"),
            agent_session_id: AgentSessionId::from("agent-session-test"),
        }
    }

    #[tokio::test]
    async fn augmentor_emits_separate_memory_message() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("AGENTS.md"),
            "# Rules\nUse canary deploys before restarts.",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("MEMORY.md"),
            "Canary deploy before restart when the change is risky.",
        )
        .unwrap();
        let backend = Arc::new(MemoryCoreBackend::new(
            dir.path().to_path_buf(),
            Default::default(),
        ));
        let augmentor = WorkspaceMemoryRecallAugmentor::new(backend);

        let augmented = augmentor
            .augment_user_message(
                &context(),
                Message::user("Should I use a canary deploy before restart?"),
            )
            .await
            .unwrap();

        assert_eq!(augmented.prefix_messages.len(), 1);
        assert_eq!(augmented.prefix_messages[0].role, MessageRole::User);
        assert_eq!(
            augmented.message.text_content(),
            "Should I use a canary deploy before restart?"
        );
        let recall = augmented.prefix_messages[0].text_content();
        assert!(recall.contains("Relevant workspace memory"));
        assert!(recall.contains("Use these notes only if they are relevant."));
        assert!(recall.contains("Ignore this recall message"));
        assert!(recall.contains("AGENTS.md") || recall.contains("MEMORY.md"));
        assert_ne!(recall, augmented.message.text_content());
        assert!(
            augmented.prefix_messages[0]
                .metadata
                .contains_key(WORKSPACE_MEMORY_RECALL_METADATA_KEY)
        );
    }

    #[test]
    fn recall_search_queries_strip_stopwords_from_questions() {
        assert_eq!(
            build_search_queries("Should I use a canary deploy before restart?"),
            vec![
                "canary deploy before restart".to_string(),
                "canary deploy restart".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn augmentor_skips_short_queries() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "# Rules\nUse canary deploys.").unwrap();
        let backend = Arc::new(MemoryCoreBackend::new(
            dir.path().to_path_buf(),
            Default::default(),
        ));
        let augmentor = WorkspaceMemoryRecallAugmentor::new(backend);

        let augmented = augmentor
            .augment_user_message(&context(), Message::user("deploy?"))
            .await
            .unwrap();

        assert!(augmented.prefix_messages.is_empty());
        assert_eq!(augmented.message.text_content(), "deploy?");
    }

    #[tokio::test]
    async fn augmentor_prioritizes_current_session_working_memory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("MEMORY.md"),
            "Durable note: deploy checklists should mention restarts.",
        )
        .unwrap();
        let backend = Arc::new(MemoryCoreBackend::new(
            dir.path().to_path_buf(),
            Default::default(),
        ));
        backend
            .record(MemoryRecordRequest {
                scope: MemoryScope::Working,
                title: "Session continuation snapshot".to_string(),
                content: "Current state: canary deploy before restart because production rollback is sensitive.".to_string(),
                memory_type: None,
                description: Some(
                    "Latest continuation snapshot for the active session.".to_string(),
                ),
                layer: None,
                tags: vec!["compaction".to_string()],
                session_id: Some(SessionId::from("session-test")),
                agent_session_id: Some(AgentSessionId::from("agent-session-test")),
                agent_name: None,
                task_id: None,
            })
            .await
            .unwrap();
        let augmentor = WorkspaceMemoryRecallAugmentor::new(backend);

        let augmented = augmentor
            .augment_user_message(
                &context(),
                Message::user("Should I do a canary deploy before restart?"),
            )
            .await
            .unwrap();

        let recall = augmented.prefix_messages[0].text_content();
        let working_path = ".nanoclaw/memory/working/agent-sessions/agent-session-test.md";
        let working_index = recall.find(working_path).unwrap();
        if let Some(durable_index) = recall.find("MEMORY.md") {
            assert!(working_index < durable_index);
        }
    }
}
