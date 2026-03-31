use crate::{ModelBackend, Result};
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::json;
use std::sync::Arc;
use types::{AgentCoreError, AgentSessionId, ModelEvent, ModelRequest, SessionId, TurnId};

#[derive(Clone, Debug)]
pub struct CompactionConfig {
    pub enabled: bool,
    pub context_window_tokens: usize,
    pub trigger_tokens: usize,
    pub preserve_recent_messages: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            context_window_tokens: 200_000,
            trigger_tokens: 150_000,
            preserve_recent_messages: 8,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompactionRequest {
    pub session_id: SessionId,
    pub agent_session_id: AgentSessionId,
    pub turn_id: TurnId,
    pub messages: Vec<types::Message>,
    pub visible_messages: Vec<types::Message>,
    pub instructions: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CompactionResult {
    pub summary: String,
}

#[async_trait]
pub trait ConversationCompactor: Send + Sync {
    async fn compact(&self, request: CompactionRequest) -> Result<CompactionResult>;
}

pub struct NoopConversationCompactor;

#[async_trait]
impl ConversationCompactor for NoopConversationCompactor {
    async fn compact(&self, _request: CompactionRequest) -> Result<CompactionResult> {
        Err(AgentCoreError::ModelBackend("compaction is not configured".to_string()).into())
    }
}

pub struct ModelConversationCompactor {
    backend: Arc<dyn ModelBackend>,
}

const SESSION_MEMORY_SECTION_HINTS: &[&str] = &[
    "Session Title",
    "Current State",
    "Task specification",
    "Files and Functions",
    "Workflow",
    "Errors & Corrections",
    "Codebase and System Documentation",
    "Learnings",
    "Key results",
    "Worklog",
];

impl ModelConversationCompactor {
    #[must_use]
    pub fn new(backend: Arc<dyn ModelBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl ConversationCompactor for ModelConversationCompactor {
    async fn compact(&self, request: CompactionRequest) -> Result<CompactionResult> {
        // The compaction output is also persisted as session-scoped working
        // memory, so keep the headings compatible with Claude-style session
        // notes instead of emitting an ad hoc summary shape.
        let mut instructions = vec![
            "Summarize the conversation so an agent can continue from the summary alone.".to_string(),
            "Preserve the user's goals, important facts, file paths, edits made, tool outcomes, constraints, open questions, and pending next steps.".to_string(),
            format!(
                "Format the result as compact Markdown using these section headers when they have material content: {}.",
                SESSION_MEMORY_SECTION_HINTS.join(", ")
            ),
            "Always include Current State. Keep Worklog terse when present, and prefer omitting empty sections instead of adding filler.".to_string(),
            "Be concise but specific. Do not mention that this is a summary.".to_string(),
        ];
        if let Some(extra) = request.instructions {
            instructions.push(extra);
        }

        let mut stream = self
            .backend
            .stream_turn(ModelRequest {
                session_id: request.session_id,
                agent_session_id: request.agent_session_id,
                turn_id: request.turn_id,
                instructions,
                messages: request.messages,
                tools: Vec::new(),
                additional_context: Vec::new(),
                continuation: None,
                metadata: json!({ "agent_core": { "purpose": "compaction" } }),
            })
            .await?;

        let mut assistant_text = String::new();
        while let Some(event) = stream.next().await {
            match event? {
                ModelEvent::TextDelta { delta } => assistant_text.push_str(&delta),
                ModelEvent::ResponseComplete { .. } => {}
                ModelEvent::ToolCallRequested { call } => {
                    return Err(AgentCoreError::ModelBackend(format!(
                        "compaction unexpectedly requested tool `{}`",
                        call.tool_name
                    ))
                    .into());
                }
                ModelEvent::Error { message } => {
                    return Err(AgentCoreError::ModelBackend(message).into());
                }
            }
        }

        let summary = assistant_text.trim().to_string();
        if summary.is_empty() {
            return Err(AgentCoreError::ModelBackend(
                "compaction returned an empty summary".to_string(),
            )
            .into());
        }

        Ok(CompactionResult { summary })
    }
}

#[must_use]
pub fn estimate_prompt_tokens(
    instructions: &[String],
    messages: &[types::Message],
    tools: &[types::ToolSpec],
    additional_context: &[String],
) -> usize {
    let mut chars = 0usize;
    chars += instructions.iter().map(|value| value.len()).sum::<usize>();
    chars += additional_context
        .iter()
        .map(|value| value.len())
        .sum::<usize>();
    chars += messages
        .iter()
        .map(|message| message.text_content().len() + 32)
        .sum::<usize>();
    chars += tools
        .iter()
        .map(|tool| {
            let schema_chars = tool
                .input_schema
                .as_ref()
                .map(ToString::to_string)
                .map_or(0, |schema| schema.len());
            tool.name.as_str().len() + tool.description.len() + schema_chars
        })
        .sum::<usize>();
    chars.div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::{CompactionRequest, ConversationCompactor, ModelConversationCompactor};
    use crate::{ModelBackend, Result};
    use async_trait::async_trait;
    use futures::{StreamExt, stream, stream::BoxStream};
    use std::sync::{Arc, Mutex};
    use types::{AgentSessionId, Message, ModelEvent, ModelRequest, SessionId, TurnId};

    #[derive(Clone, Default)]
    struct RecordingCompactionBackend {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    impl RecordingCompactionBackend {
        fn requests(&self) -> Vec<ModelRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ModelBackend for RecordingCompactionBackend {
        async fn stream_turn(
            &self,
            request: ModelRequest,
        ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
            self.requests.lock().unwrap().push(request);
            Ok(stream::iter(vec![
                Ok(ModelEvent::TextDelta {
                    delta: "# Current State\n\nKeep the deploy guardrails.".to_string(),
                }),
                Ok(ModelEvent::ResponseComplete {
                    stop_reason: Some("stop".to_string()),
                    message_id: None,
                    continuation: None,
                    usage: None,
                    reasoning: Vec::new(),
                }),
            ])
            .boxed())
        }
    }

    #[tokio::test]
    async fn model_compactor_requests_claude_style_session_memory_sections() {
        let backend = Arc::new(RecordingCompactionBackend::default());
        let compactor = ModelConversationCompactor::new(backend.clone());

        let result = compactor
            .compact(CompactionRequest {
                session_id: SessionId::from("session-1"),
                agent_session_id: AgentSessionId::from("agent-session-1"),
                turn_id: TurnId::new(),
                messages: vec![Message::user("summarize the deploy fixes")],
                visible_messages: vec![Message::user("summarize the deploy fixes")],
                instructions: None,
            })
            .await
            .unwrap();

        assert!(result.summary.contains("# Current State"));

        let recorded = backend.requests();
        assert_eq!(recorded.len(), 1);
        let joined = recorded[0].instructions.join("\n");
        assert!(joined.contains("Session Title"));
        assert!(joined.contains("Current State"));
        assert!(joined.contains("Task specification"));
        assert!(joined.contains("Files and Functions"));
        assert!(joined.contains("Workflow"));
        assert!(joined.contains("Errors & Corrections"));
        assert!(joined.contains("Codebase and System Documentation"));
        assert!(joined.contains("Learnings"));
        assert!(joined.contains("Key results"));
        assert!(joined.contains("Worklog"));
        assert!(joined.contains("Always include Current State"));
    }
}
