use tokio::sync::mpsc;
use types::new_opaque_id;

use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};

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
pub struct RuntimeCommandId(String);

impl RuntimeCommandId {
    fn new() -> Self {
        Self(new_opaque_id())
    }
}

impl fmt::Display for RuntimeCommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedRuntimeCommand {
    pub id: RuntimeCommandId,
    pub command: RuntimeCommand,
}

const RUNTIME_COMMAND_QUEUE_CAPACITY: usize = 64;

pub struct RuntimeCommandQueue {
    sender: mpsc::Sender<QueuedRuntimeCommand>,
    receiver: mpsc::Receiver<QueuedRuntimeCommand>,
    len: AtomicUsize,
}

impl RuntimeCommandQueue {
    #[must_use]
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel(RUNTIME_COMMAND_QUEUE_CAPACITY);
        Self {
            sender,
            receiver,
            len: AtomicUsize::new(0),
        }
    }

    pub async fn push(&self, command: RuntimeCommand) -> QueuedRuntimeCommand {
        let queued = QueuedRuntimeCommand {
            id: RuntimeCommandId::new(),
            command,
        };
        // Queue coordination is message-passing, not shared mutable state. A
        // bounded channel prevents unbounded growth when the operator keeps
        // enqueueing prompts faster than the runtime can consume them.
        self.sender
            .send(queued.clone())
            .await
            .expect("runtime command queue receiver dropped");
        self.len.fetch_add(1, Ordering::Relaxed);
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

    pub fn pop_next(&mut self) -> Option<QueuedRuntimeCommand> {
        let next = self.receiver.try_recv().ok();
        if next.is_some() {
            self.len.fetch_sub(1, Ordering::Relaxed);
        }
        next
    }

    pub async fn len(&self) -> usize {
        self.len.load(Ordering::Relaxed)
    }

    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
}

impl Default for RuntimeCommandQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{RuntimeCommand, RuntimeCommandQueue};

    #[tokio::test]
    async fn queue_preserves_fifo_order() {
        let mut queue = RuntimeCommandQueue::new();
        let first = queue.push_prompt("one").await;
        let second = queue
            .push_steer("use concise output", Some("manual".to_string()))
            .await;

        assert_eq!(queue.len().await, 2);

        let popped_first = queue.pop_next().unwrap();
        let popped_second = queue.pop_next().unwrap();
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
