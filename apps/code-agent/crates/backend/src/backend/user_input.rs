use crate::frontend_contract::user_input_prompt_from_request;
use crate::interaction::{UserInputPrompt, UserInputSubmission};
use agent::tools::{
    Result as ToolResult, ToolError, UserInputHandler, UserInputRequest, UserInputResponse,
};
use async_trait::async_trait;
use std::sync::{Arc, RwLock};
use tokio::sync::oneshot;

#[derive(Default)]
struct UserInputCoordinatorState {
    prompt: Option<UserInputPrompt>,
    responder: Option<oneshot::Sender<ToolResult<UserInputResponse>>>,
}

/// Request-user-input prompts follow the same host-owned handshake pattern as
/// approvals: the backend stores the pending prompt and a one-shot responder so
/// the foreground UI can answer without touching runtime internals directly.
#[derive(Clone, Default)]
pub struct UserInputCoordinator {
    inner: Arc<RwLock<UserInputCoordinatorState>>,
}

impl UserInputCoordinator {
    pub fn snapshot(&self) -> Option<UserInputPrompt> {
        self.inner.read().unwrap().prompt.clone()
    }

    pub fn resolve(&self, submission: UserInputSubmission) -> bool {
        let mut inner = self.inner.write().unwrap();
        let responder = inner.responder.take();
        inner.prompt = None;
        if let Some(responder) = responder {
            let response = UserInputResponse {
                answers: submission
                    .answers
                    .into_iter()
                    .map(|(key, answer)| {
                        (
                            key,
                            agent::tools::UserInputAnswer {
                                answers: answer.answers,
                            },
                        )
                    })
                    .collect(),
            };
            let _ = responder.send(Ok(response));
            true
        } else {
            false
        }
    }

    pub fn cancel(&self, reason: impl Into<String>) -> bool {
        let mut inner = self.inner.write().unwrap();
        let responder = inner.responder.take();
        inner.prompt = None;
        if let Some(responder) = responder {
            let _ = responder.send(Err(ToolError::invalid_state(reason.into())));
            true
        } else {
            false
        }
    }

    fn present(
        &self,
        prompt: UserInputPrompt,
        responder: oneshot::Sender<ToolResult<UserInputResponse>>,
    ) {
        let mut inner = self.inner.write().unwrap();
        inner.prompt = Some(prompt);
        inner.responder = Some(responder);
    }
}

pub struct SessionUserInputHandler {
    coordinator: UserInputCoordinator,
}

impl SessionUserInputHandler {
    pub fn new(coordinator: UserInputCoordinator) -> Self {
        Self { coordinator }
    }
}

#[async_trait]
impl UserInputHandler for SessionUserInputHandler {
    async fn request_input(&self, request: UserInputRequest) -> ToolResult<UserInputResponse> {
        let (tx, rx) = oneshot::channel();
        self.coordinator.present(
            user_input_prompt_from_request(agent::new_opaque_id().to_string(), request),
            tx,
        );
        match rx.await {
            Ok(result) => result,
            Err(error) => Err(ToolError::invalid_state(format!(
                "request_user_input dialog closed unexpectedly: {error}"
            ))),
        }
    }
}

pub struct NonInteractiveUserInputHandler {
    reason: String,
}

impl NonInteractiveUserInputHandler {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

#[async_trait]
impl UserInputHandler for NonInteractiveUserInputHandler {
    async fn request_input(&self, _request: UserInputRequest) -> ToolResult<UserInputResponse> {
        Err(ToolError::invalid_state(self.reason.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::{SessionUserInputHandler, UserInputCoordinator, UserInputPrompt};
    use crate::interaction::{UserInputAnswer, UserInputSubmission};
    use agent::tools::{UserInputHandler, UserInputRequest};
    use std::collections::BTreeMap;
    use tokio::task::yield_now;

    #[tokio::test]
    async fn coordinator_round_trips_user_input_answers() {
        let coordinator = UserInputCoordinator::default();
        let handler = SessionUserInputHandler::new(coordinator.clone());

        let task = tokio::spawn(async move {
            handler
                .request_input(UserInputRequest {
                    questions: vec![agent::tools::UserInputQuestion {
                        id: "scope_choice".to_string(),
                        header: "Scope".to_string(),
                        question: "Which scope should I target?".to_string(),
                        options: vec![
                            agent::tools::UserInputOption {
                                label: "Runtime (Recommended)".to_string(),
                                description: "Touches only substrate runtime code.".to_string(),
                            },
                            agent::tools::UserInputOption {
                                label: "Host".to_string(),
                                description: "Touches the code-agent app.".to_string(),
                            },
                        ],
                    }],
                })
                .await
        });

        let prompt = loop {
            if let Some(prompt) = coordinator.snapshot() {
                break prompt;
            }
            yield_now().await;
        };
        assert_eq!(prompt.questions.len(), 1);
        assert_eq!(prompt.questions[0].id, "scope_choice");
        assert_ne!(
            prompt,
            UserInputPrompt {
                prompt_id: String::new(),
                questions: Vec::new(),
            }
        );

        assert!(coordinator.resolve(UserInputSubmission {
            answers: BTreeMap::from([(
                "scope_choice".to_string(),
                UserInputAnswer {
                    answers: vec!["Runtime (Recommended)".to_string()],
                },
            )]),
        }));

        let result = task.await.unwrap().unwrap();
        assert_eq!(
            result.answers["scope_choice"].answers,
            vec!["Runtime (Recommended)".to_string()]
        );
    }
}
