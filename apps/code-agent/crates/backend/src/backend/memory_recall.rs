use agent::memory::{
    MemoryBackend, MemoryDocument, MemoryGetRequest, MemoryListEntry, MemoryListRequest,
    MemoryScope, MemorySearchHit, MemorySearchRequest,
};
use agent::runtime::{AugmentedUserMessage, UserMessageAugmentationContext, UserMessageAugmentor};
use agent::types::{Message, MessagePart, MessageRole};
use async_trait::async_trait;
use serde_json::json;
use std::collections::BTreeSet;
use std::path::Path;
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
const RECALL_WORKING_SESSION_PATH_PREFIX: &str = ".nanoclaw/memory/working/sessions/";
const RECALL_MAX_SNIPPET_CHARS: usize = 220;
const RECALL_MAX_BLOCK_CHARS: usize = 1_400;
const RECALL_QUERY_TERM_LIMIT: usize = 5;
const RECALL_QUERY_FALLBACK_TERM_LIMIT: usize = 3;
const RECALL_DURABLE_LIST_LIMIT: usize = 128;
const RECALL_DURABLE_SELECTION_LIMIT: usize = 5;
const RECALL_DURABLE_LINE_LIMIT: usize = 40;
const RECALL_DURABLE_SELECTOR_BACKEND: &str = "memory-header-select";

#[derive(Clone)]
pub struct WorkspaceMemoryRecallAugmentor {
    backend: Arc<dyn MemoryBackend>,
    timeout_ms: u64,
}

impl WorkspaceMemoryRecallAugmentor {
    pub fn new(backend: Arc<dyn MemoryBackend>) -> Self {
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

        // Query the stable per-session continuation snapshot first so the
        // newest compacted handoff wins even after agent-session rotation.
        if let Some((name, working_hits)) = self
            .search_hits(
                query,
                Some(RECALL_WORKING_SESSION_PATH_PREFIX.to_string()),
                vec![MemoryScope::Working],
                Some(context.session_id.clone()),
                1,
                started_at,
            )
            .await
        {
            backend_name = Some(name);
            merge_recall_hits(&mut collected, working_hits);
        }
        if collected.is_empty() {
            // Fall back to broader working-memory search for pre-migration
            // agent-session files or future non-session scratchpad records.
            if let Some((name, working_hits)) = self
                .search_hits(
                    query,
                    None,
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
        }
        if collected.len() < RECALL_LIMIT {
            if let Some((name, durable_hits)) = self.select_durable_hits(query, started_at).await {
                backend_name.get_or_insert(name);
                merge_recall_hits(&mut collected, durable_hits);
            } else if let Some((name, durable_hits)) = self
                .search_hits(
                    query,
                    None,
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

    async fn select_durable_hits(
        &self,
        query: &str,
        started_at: std::time::Instant,
    ) -> Option<(String, Vec<MemorySearchHit>)> {
        let query_terms = durable_selector_terms(query);
        if query_terms.len() < 2 {
            return None;
        }

        let remaining = Duration::from_millis(self.timeout_ms).saturating_sub(started_at.elapsed());
        if remaining.is_zero() {
            debug!("workspace memory recall timed out before durable selection completed");
            return None;
        }

        let response = match timeout(
            remaining,
            self.backend.list(MemoryListRequest {
                limit: Some(RECALL_DURABLE_LIST_LIMIT),
                path_prefix: None,
                scopes: Some(vec![MemoryScope::Procedural, MemoryScope::Semantic]),
                types: None,
                tags: None,
                session_id: None,
                agent_session_id: None,
                agent_name: None,
                task_id: None,
                include_stale: Some(false),
            }),
        )
        .await
        {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => {
                warn!(error = %error, "workspace durable memory listing failed");
                return None;
            }
            Err(_) => {
                debug!("workspace memory recall timed out during durable listing");
                return None;
            }
        };

        let selected = select_relevant_durable_memories(&response.entries, &query_terms);
        if selected.is_empty() {
            return None;
        }

        let mut hits = Vec::new();
        for candidate in selected {
            let remaining =
                Duration::from_millis(self.timeout_ms).saturating_sub(started_at.elapsed());
            if remaining.is_zero() {
                debug!("workspace memory recall timed out during durable reads");
                break;
            }
            let document = match timeout(
                remaining,
                self.backend.get(MemoryGetRequest {
                    path: candidate.entry.path.clone(),
                    start_line: Some(1),
                    line_count: Some(RECALL_DURABLE_LINE_LIMIT),
                }),
            )
            .await
            {
                Ok(Ok(document)) => document,
                Ok(Err(error)) => {
                    warn!(
                        error = %error,
                        path = %candidate.entry.path,
                        "workspace durable memory read failed"
                    );
                    continue;
                }
                Err(_) => {
                    debug!("workspace memory recall timed out during durable read");
                    break;
                }
            };
            let Some(snippet) = recall_snippet_from_document(&document, &query_terms) else {
                continue;
            };
            hits.push(MemorySearchHit {
                hit_id: format!("selected:{}", document.snapshot_id),
                path: document.path.clone(),
                start_line: document.resolved_start_line,
                end_line: document.resolved_end_line,
                score: candidate.score as f64,
                snippet,
                document_metadata: document.metadata.clone(),
                metadata: Default::default(),
            });
            if hits.len() == RECALL_LIMIT {
                break;
            }
        }

        (!hits.is_empty()).then(|| (RECALL_DURABLE_SELECTOR_BACKEND.to_string(), hits))
    }

    async fn search_hits(
        &self,
        query: &str,
        path_prefix: Option<String>,
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
                path_prefix: path_prefix.clone(),
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

#[derive(Clone)]
struct DurableMemoryCandidate<'a> {
    entry: &'a MemoryListEntry,
    score: usize,
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

fn durable_selector_terms(query: &str) -> Vec<String> {
    let raw_terms = tokenize_query(query);
    if raw_terms.len() < 2 {
        return Vec::new();
    }
    let keyword_terms = raw_terms
        .iter()
        .filter(|term| !is_query_stop_word(term))
        .cloned()
        .collect::<Vec<_>>();
    if keyword_terms.len() >= 2 {
        keyword_terms
    } else {
        raw_terms
    }
}

fn select_relevant_durable_memories<'a>(
    entries: &'a [MemoryListEntry],
    query_terms: &[String],
) -> Vec<DurableMemoryCandidate<'a>> {
    let mut scored = entries
        .iter()
        .filter(|entry| !is_recall_primer_entrypoint(entry))
        .filter_map(|entry| {
            durable_memory_candidate_score(entry, query_terms)
                .map(|score| DurableMemoryCandidate { entry, score })
        })
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.entry.path.cmp(&right.entry.path))
    });
    scored.truncate(RECALL_DURABLE_SELECTION_LIMIT);
    scored
}

fn durable_memory_candidate_score(
    entry: &MemoryListEntry,
    query_terms: &[String],
) -> Option<usize> {
    let title = entry.title.to_ascii_lowercase();
    let description = entry
        .metadata
        .description
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let path = entry.path.to_ascii_lowercase();
    let tags = entry
        .metadata
        .tags
        .iter()
        .map(|tag| tag.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let type_term = entry
        .metadata
        .memory_type
        .map(|memory_type| memory_type.as_str().to_string());

    let title_tokens = tokenize_query(&title);
    let description_tokens = tokenize_query(&description);
    let path_tokens = tokenize_query(&path);
    let tag_tokens = tags
        .iter()
        .flat_map(|tag| tokenize_query(tag))
        .collect::<Vec<_>>();

    let unique_overlap = query_terms
        .iter()
        .filter(|term| {
            title_tokens.contains(term)
                || description_tokens.contains(term)
                || path_tokens.contains(term)
                || tag_tokens.contains(term)
                || type_term
                    .as_deref()
                    .is_some_and(|value| value == term.as_str())
        })
        .count();
    let phrase_match = query_term_phrases(query_terms).into_iter().any(|phrase| {
        title.contains(&phrase)
            || description.contains(&phrase)
            || path.contains(&phrase)
            || tags.iter().any(|tag| tag.contains(&phrase))
    });
    if unique_overlap < 2 && !phrase_match {
        return None;
    }

    let score = unique_overlap * 100
        + count_overlap(query_terms, &title_tokens) * 40
        + count_overlap(query_terms, &description_tokens) * 24
        + count_overlap(query_terms, &tag_tokens) * 18
        + count_overlap(query_terms, &path_tokens) * 12
        + usize::from(phrase_match) * 30
        + usize::from(
            type_term
                .as_deref()
                .is_some_and(|value| query_terms.iter().any(|term| term == value)),
        ) * 8;
    Some(score)
}

fn query_term_phrases(query_terms: &[String]) -> Vec<String> {
    let mut phrases = Vec::new();
    for window in query_terms.windows(2) {
        phrases.push(window.join(" "));
    }
    if query_terms.len() >= 3 {
        phrases.push(
            query_terms
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    phrases
}

fn count_overlap(query_terms: &[String], field_terms: &[String]) -> usize {
    query_terms
        .iter()
        .filter(|term| field_terms.contains(term))
        .count()
}

fn is_recall_primer_entrypoint(entry: &MemoryListEntry) -> bool {
    if entry.metadata.layer == "auto-memory-index" {
        return true;
    }
    matches!(
        Path::new(&entry.path)
            .file_name()
            .and_then(|name| name.to_str()),
        Some("AGENTS.md" | "MEMORY.md")
    )
}

fn recall_snippet_from_document(
    document: &MemoryDocument,
    query_terms: &[String],
) -> Option<String> {
    let mut matching = Vec::new();
    let mut fallback = Vec::new();

    for line in document.text.lines() {
        let stripped = strip_numbered_line(line);
        if stripped.is_empty()
            || stripped.starts_with("# ")
            || (stripped.starts_with('_') && stripped.ends_with('_'))
        {
            continue;
        }

        let overlap = count_overlap(query_terms, &tokenize_query(stripped));
        if overlap > 0 {
            matching.push((overlap, stripped.to_string()));
        } else {
            fallback.push(stripped.to_string());
        }
    }

    matching.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    let mut lines = matching
        .into_iter()
        .map(|(_, line)| line)
        .take(2)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        lines.extend(fallback.into_iter().take(2));
    }
    if lines.is_empty() {
        return None;
    }
    Some(normalize_snippet(&lines.join(" ")))
}

fn strip_numbered_line(line: &str) -> &str {
    let trimmed = line.trim();
    let Some((prefix, rest)) = trimmed.split_once(':') else {
        return trimmed;
    };
    if prefix.chars().all(|ch| ch.is_ascii_digit()) {
        rest.trim()
    } else {
        trimmed
    }
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
    use agent::memory::{
        MemoryBackend, MemoryCoreBackend, MemoryRecordMode, MemoryRecordRequest, MemoryScope,
    };
    use agent::runtime::{UserMessageAugmentationContext, UserMessageAugmentor};
    use agent::types::{AgentSessionId, Message, MessageRole, SessionId};
    use std::path::Path;
    use std::sync::Arc;

    fn context() -> UserMessageAugmentationContext {
        UserMessageAugmentationContext {
            session_id: SessionId::from("session-test"),
            agent_session_id: AgentSessionId::from("agent-session-test"),
        }
    }

    fn write_managed_durable_memory(
        workspace_root: &Path,
        scope: MemoryScope,
        file_name: &str,
        title: &str,
        description: &str,
        tags: &[&str],
        body: &str,
    ) -> String {
        let scope_dir = match scope {
            MemoryScope::Procedural => ".nanoclaw/memory/procedural",
            MemoryScope::Semantic => ".nanoclaw/memory/semantic",
            other => panic!("unsupported durable scope for test fixture: {other:?}"),
        };
        let relative_path = format!("{scope_dir}/{file_name}");
        let absolute_path = workspace_root.join(&relative_path);
        std::fs::create_dir_all(absolute_path.parent().unwrap()).unwrap();
        let tags_block = tags
            .iter()
            .map(|tag| format!("  - {tag}"))
            .collect::<Vec<_>>()
            .join("\n");
        let content = format!(
            "---\nscope: {scope}\ntype: project\ndescription: {description}\ntags:\n{tags_block}\n---\n# {title}\n{body}\n",
            scope = scope.as_str(),
        );
        std::fs::write(&absolute_path, content).unwrap();
        relative_path
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
                mode: MemoryRecordMode::Replace,
                memory_type: None,
                description: Some(
                    "Latest continuation snapshot for the active session.".to_string(),
                ),
                layer: Some("session".to_string()),
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
        let working_path = ".nanoclaw/memory/working/sessions/session-test.md";
        let working_index = recall.find(working_path).unwrap();
        if let Some(durable_index) = recall.find("MEMORY.md") {
            assert!(working_index < durable_index);
        }
    }

    #[tokio::test]
    async fn durable_recall_prefers_topic_files_over_primer_entrypoints() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("MEMORY.md"),
            "General deploy reminders live here, but detailed runbooks live under managed memory.",
        )
        .unwrap();
        let stored_path = write_managed_durable_memory(
            dir.path(),
            MemoryScope::Procedural,
            "canary-restart-runbook.md",
            "Canary restart runbook",
            "Canary deploy before restart for risky rollout windows.",
            &["deploy", "restart"],
            "Use a canary deploy before restart when production risk is elevated.",
        );
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

        let recall = augmented.prefix_messages[0].text_content();
        assert!(recall.contains(&stored_path));
        assert!(!recall.contains("- MEMORY.md ["));
        assert!(!recall.contains("- AGENTS.md ["));
    }

    #[tokio::test]
    async fn durable_recall_falls_back_to_body_search_when_headers_do_not_match() {
        let dir = tempfile::tempdir().unwrap();
        let stored_path = write_managed_durable_memory(
            dir.path(),
            MemoryScope::Semantic,
            "deployment-notes.md",
            "Deployment notes",
            "Rollout guidance.",
            &["ops"],
            "Use zebra restart sequencing to avoid duplicate worker claims.",
        );
        let backend = Arc::new(MemoryCoreBackend::new(
            dir.path().to_path_buf(),
            Default::default(),
        ));
        let augmentor = WorkspaceMemoryRecallAugmentor::new(backend);

        let augmented = augmentor
            .augment_user_message(
                &context(),
                Message::user("When should I use zebra restart sequencing?"),
            )
            .await
            .unwrap();

        let recall = augmented.prefix_messages[0].text_content();
        assert!(recall.contains(&stored_path));
        assert!(recall.contains("zebra restart sequencing"));
    }
}
