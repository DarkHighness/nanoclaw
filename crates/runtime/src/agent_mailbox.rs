use tokio::sync::mpsc;
use types::Message;

#[derive(Clone, Debug)]
pub enum AgentControlMessage {
    Input { message: Message },
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

    pub fn send_input(
        &self,
        message: Message,
    ) -> Result<(), mpsc::error::SendError<AgentControlMessage>> {
        self.tx.send(AgentControlMessage::Input { message })
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
    use types::Message;

    #[tokio::test]
    async fn mailbox_delivers_input_then_cancel() {
        let (mailbox, mut rx) = agent_mailbox_channel();
        mailbox.send_input(Message::user("focus")).unwrap();
        mailbox.cancel(Some("stop".to_string())).unwrap();

        match rx.recv().await.unwrap() {
            AgentControlMessage::Input { message } => {
                assert_eq!(message.text_content(), "focus");
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
