use crate::Result;
use async_trait::async_trait;
use types::{AgentSessionId, Message, SessionId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserMessageAugmentationContext {
    pub session_id: SessionId,
    pub agent_session_id: AgentSessionId,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AugmentedUserMessage {
    // Synthetic context must stay in separate transcript messages so the
    // operator's original prompt bytes remain stable for hooks, auditing, and
    // provider-visible request construction.
    pub prefix_messages: Vec<Message>,
    pub message: Message,
}

impl AugmentedUserMessage {
    #[must_use]
    pub fn unchanged(message: Message) -> Self {
        Self {
            prefix_messages: Vec::new(),
            message,
        }
    }
}

#[async_trait]
pub trait UserMessageAugmentor: Send + Sync {
    async fn augment_user_message(
        &self,
        context: &UserMessageAugmentationContext,
        message: Message,
    ) -> Result<AugmentedUserMessage>;
}
