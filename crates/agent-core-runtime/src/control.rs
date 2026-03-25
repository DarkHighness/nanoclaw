use agent_core_types::new_opaque_id;
use tokio::sync::Mutex;

use std::collections::VecDeque;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeCommand {
    Prompt {
        prompt: String,
    },
    Steer {
        message: String,
        reason: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedRuntimeCommand {
    pub id: String,
    pub command: RuntimeCommand,
}

#[derive(Clone, Default)]
pub struct RuntimeCommandQueue {
    inner: Arc<Mutex<VecDeque<QueuedRuntimeCommand>>>,
}

impl RuntimeCommandQueue {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn push(&self, command: RuntimeCommand) -> QueuedRuntimeCommand {
        let queued = QueuedRuntimeCommand {
            id: new_opaque_id(),
            command,
        };
        self.inner.lock().await.push_back(queued.clone());
        queued
    }

    pub async fn push_prompt(&self, prompt: impl Into<String>) -> QueuedRuntimeCommand {
        self.push(RuntimeCommand::Prompt {
            prompt: prompt.into(),
        })
        .await
    }

    pub async fn push_steer(
        &self,
        message: impl Into<String>,
        reason: Option<String>,
    ) -> QueuedRuntimeCommand {
        self.push(RuntimeCommand::Steer {
            message: message.into(),
            reason,
        })
        .await
    }

    pub async fn pop_next(&self) -> Option<QueuedRuntimeCommand> {
        self.inner.lock().await.pop_front()
    }

    pub async fn len(&self) -> usize {
        self.inner.lock().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.inner.lock().await.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{RuntimeCommand, RuntimeCommandQueue};

    #[tokio::test]
    async fn queue_preserves_fifo_order() {
        let queue = RuntimeCommandQueue::new();
        let first = queue.push_prompt("one").await;
        let second = queue
            .push_steer("use concise output", Some("manual".to_string()))
            .await;

        assert_eq!(queue.len().await, 2);

        let popped_first = queue.pop_next().await.unwrap();
        let popped_second = queue.pop_next().await.unwrap();
        assert_eq!(popped_first.id, first.id);
        assert_eq!(popped_second.id, second.id);
        assert!(matches!(
            popped_second.command,
            RuntimeCommand::Steer { message, reason }
                if message == "use concise output" && reason.as_deref() == Some("manual")
        ));
        assert!(queue.is_empty().await);
    }
}
