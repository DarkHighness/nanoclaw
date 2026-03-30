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

impl ModelConversationCompactor {
    #[must_use]
    pub fn new(backend: Arc<dyn ModelBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl ConversationCompactor for ModelConversationCompactor {
    async fn compact(&self, request: CompactionRequest) -> Result<CompactionResult> {
        let mut instructions = vec![
            "Summarize the conversation so an agent can continue from the summary alone.".to_string(),
            "Preserve the user's goals, important facts, file paths, edits made, tool outcomes, constraints, open questions, and pending next steps.".to_string(),
            "Be concise but specific. Use short section headers. Do not mention that this is a summary.".to_string(),
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
