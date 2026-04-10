use types::new_opaque_id;
use types::{Message, SubmittedPromptSnapshot};

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq)]
pub enum RuntimeCommand {
    Prompt {
        message: Message,
        submitted_prompt: Option<SubmittedPromptSnapshot>,
    },
    Steer {
        message: String,
        reason: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RuntimeCommandId(String);

impl RuntimeCommandId {
    fn new() -> Self {
        // Queue ids are operator-facing correlation handles for queued prompts
        // and steer commands. Keeping them distinct from session/tool ids avoids
        // accidentally reusing substrate-wide ids for a purely local control
        // plane.
        Self(new_opaque_id())
    }
}

impl fmt::Display for RuntimeCommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for RuntimeCommandId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for RuntimeCommandId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeCommandLane {
    Idle,
    SafePoint,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QueuedRuntimeCommand {
    pub id: RuntimeCommandId,
    pub command: RuntimeCommand,
    pub lane: RuntimeCommandLane,
}

#[derive(Clone, Default)]
pub struct RuntimeControlPlane {
    inner: Arc<Mutex<VecDeque<QueuedRuntimeCommand>>>,
}

impl RuntimeControlPlane {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&self, command: RuntimeCommand, lane: RuntimeCommandLane) -> QueuedRuntimeCommand {
        let queued = QueuedRuntimeCommand {
            id: RuntimeCommandId::new(),
            command,
            lane,
        };
        // The control plane keeps pending prompt/steer items in one mutable
        // queue because operators can inspect, edit, and withdraw entries
        // before the runtime consumes them. A plain channel would hide queued
        // state from those mutations.
        self.inner.lock().unwrap().push_back(queued.clone());
        queued
    }

    pub fn push_prompt(&self, message: Message) -> QueuedRuntimeCommand {
        self.push_prompt_with_snapshot(message, None)
    }

    pub fn push_prompt_with_snapshot(
        &self,
        message: Message,
        submitted_prompt: Option<SubmittedPromptSnapshot>,
    ) -> QueuedRuntimeCommand {
        self.push(
            RuntimeCommand::Prompt {
                message,
                submitted_prompt,
            },
            RuntimeCommandLane::Idle,
        )
    }

    pub fn push_steer(
        &self,
        message: impl Into<String>,
        reason: Option<String>,
    ) -> QueuedRuntimeCommand {
        self.push(
            RuntimeCommand::Steer {
                message: message.into(),
                reason,
            },
            RuntimeCommandLane::SafePoint,
        )
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<QueuedRuntimeCommand> {
        self.inner.lock().unwrap().iter().cloned().collect()
    }

    pub fn remove(&self, id: &RuntimeCommandId) -> Option<QueuedRuntimeCommand> {
        let mut inner = self.inner.lock().unwrap();
        let index = inner.iter().position(|queued| &queued.id == id)?;
        inner.remove(index)
    }

    pub fn update(
        &self,
        id: &RuntimeCommandId,
        new_command: RuntimeCommand,
    ) -> Option<QueuedRuntimeCommand> {
        let mut inner = self.inner.lock().unwrap();
        let queued = inner.iter_mut().find(|queued| &queued.id == id)?;
        if !matches_runtime_command_kind(&queued.command, &new_command) {
            return None;
        }
        queued.command = new_command;
        Some(queued.clone())
    }

    pub fn clear(&self) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let cleared = inner.len();
        inner.clear();
        cleared
    }

    pub fn pop_next(&self) -> Option<QueuedRuntimeCommand> {
        self.inner.lock().unwrap().pop_front()
    }

    pub fn pop_next_safe_point(&self) -> Option<QueuedRuntimeCommand> {
        let mut inner = self.inner.lock().unwrap();
        let index = inner
            .iter()
            .position(|queued| queued.lane == RuntimeCommandLane::SafePoint)?;
        inner.remove(index)
    }
}

fn matches_runtime_command_kind(left: &RuntimeCommand, right: &RuntimeCommand) -> bool {
    matches!(
        (left, right),
        (RuntimeCommand::Prompt { .. }, RuntimeCommand::Prompt { .. })
            | (RuntimeCommand::Steer { .. }, RuntimeCommand::Steer { .. })
    )
}

#[cfg(test)]
mod tests {
    use super::{RuntimeCommand, RuntimeCommandLane, RuntimeControlPlane};
    use types::Message;

    #[test]
    fn queue_preserves_fifo_order() {
        let queue = RuntimeControlPlane::new();
        let first = queue.push_prompt(Message::user("one"));
        let second = queue.push_prompt(Message::user("two"));

        assert_eq!(queue.len(), 2);
        assert_eq!(queue.pop_next().unwrap().id, first.id);
        assert_eq!(queue.pop_next().unwrap().id, second.id);
        assert!(queue.is_empty());
    }

    #[test]
    fn safe_point_pop_skips_idle_prompts() {
        let queue = RuntimeControlPlane::new();
        let prompt = queue.push_prompt(Message::user("one"));
        let steer = queue.push_steer("use concise output", Some("manual".to_string()));

        let popped = queue.pop_next_safe_point().unwrap();
        assert_eq!(popped.id, steer.id);
        assert_eq!(queue.snapshot(), vec![prompt]);
    }

    #[test]
    fn update_and_remove_mutate_operator_visible_queue() {
        let queue = RuntimeControlPlane::new();
        let prompt = queue.push_prompt(Message::user("draft"));
        let steer = queue.push_steer("focus", Some("manual".to_string()));

        let updated = queue
            .update(
                &prompt.id,
                RuntimeCommand::Prompt {
                    message: Message::user("edited"),
                    submitted_prompt: None,
                },
            )
            .unwrap();
        assert_eq!(updated.id, prompt.id);
        assert_eq!(updated.lane, RuntimeCommandLane::Idle);
        match updated.command {
            RuntimeCommand::Prompt { message, .. } => {
                assert_eq!(message.text_content(), "edited");
            }
            RuntimeCommand::Steer { .. } => panic!("expected queued prompt"),
        }

        let removed = queue.remove(&steer.id).unwrap();
        assert_eq!(removed.id, steer.id);
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn clear_drains_pending_commands() {
        let queue = RuntimeControlPlane::new();
        queue.push_prompt(Message::user("one"));
        queue.push_steer("use concise output", Some("manual".to_string()));

        assert_eq!(queue.clear(), 2);
        assert!(queue.pop_next().is_none());
        assert!(queue.is_empty());
    }
}
