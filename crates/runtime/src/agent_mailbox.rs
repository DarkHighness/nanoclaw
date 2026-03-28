use serde_json::Value;
use tokio::sync::mpsc;

#[derive(Clone, Debug)]
pub enum AgentControlMessage {
    Message { channel: String, payload: Value },
    Cancel { reason: Option<String> },
}

#[derive(Clone)]
pub struct AgentMailbox {
    tx: mpsc::UnboundedSender<AgentControlMessage>,
}

impl AgentMailbox {
    #[must_use]
    pub fn new(tx: mpsc::UnboundedSender<AgentControlMessage>) -> Self {
        Self { tx }
    }

    pub fn send(
        &self,
        channel: String,
        payload: Value,
    ) -> Result<(), mpsc::error::SendError<AgentControlMessage>> {
        self.tx
            .send(AgentControlMessage::Message { channel, payload })
    }

    pub fn cancel(
        &self,
        reason: Option<String>,
    ) -> Result<(), mpsc::error::SendError<AgentControlMessage>> {
        self.tx.send(AgentControlMessage::Cancel { reason })
    }
}

pub type AgentMailboxReceiver = mpsc::UnboundedReceiver<AgentControlMessage>;

#[must_use]
pub fn agent_mailbox_channel() -> (AgentMailbox, AgentMailboxReceiver) {
    let (tx, rx) = mpsc::unbounded_channel();
    (AgentMailbox::new(tx), rx)
}

#[cfg(test)]
mod tests {
    use super::{AgentControlMessage, agent_mailbox_channel};

    #[tokio::test]
    async fn mailbox_delivers_message_then_cancel() {
        let (mailbox, mut rx) = agent_mailbox_channel();
        mailbox
            .send("steer".to_string(), serde_json::json!({"text":"focus"}))
            .unwrap();
        mailbox.cancel(Some("stop".to_string())).unwrap();

        match rx.recv().await.unwrap() {
            AgentControlMessage::Message { channel, payload } => {
                assert_eq!(channel, "steer");
                assert_eq!(payload["text"], "focus");
            }
            other => panic!("unexpected message: {other:?}"),
        }
        match rx.recv().await.unwrap() {
            AgentControlMessage::Cancel { reason } => {
                assert_eq!(reason.as_deref(), Some("stop"));
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }
}
