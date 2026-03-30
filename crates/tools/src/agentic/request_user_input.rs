use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::registry::Tool;
use crate::{
    Result, ToolError, ToolExecutionContext, UserInputQuestion, UserInputRequest, UserInputResponse,
};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Serialize;
use serde_json::Value;
use types::{MessagePart, ToolCallId, ToolOutputMode, ToolResult, ToolSpec};

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct RequestUserInputToolOutput {
    question_count: usize,
    answers: UserInputResponse,
}

#[derive(Clone, Debug, Default)]
pub struct RequestUserInputTool;

impl RequestUserInputTool {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for RequestUserInputTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "request_user_input",
            "Ask the user to choose between concrete options. Questions should be short, use 2-3 mutually exclusive options, and keep the batch focused unless the decision genuinely needs several prompts.",
            serde_json::to_value(schema_for!(UserInputRequest))
                .expect("request_user_input schema"),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, true, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(RequestUserInputToolOutput))
                .expect("request_user_input output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let request: UserInputRequest = serde_json::from_value(arguments)?;
        validate_questions(&request.questions)?;
        let handler = ctx.user_input_handler.as_ref().ok_or_else(|| {
            ToolError::invalid_state(
                "request_user_input is unavailable without a host user-input handler",
            )
        })?;
        let answers = handler.request_input(request.clone()).await?;
        let structured_output = RequestUserInputToolOutput {
            question_count: request.questions.len(),
            answers: answers.clone(),
        };
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "request_user_input".into(),
            parts: vec![MessagePart::text(format_answers(&request, &answers))],
            attachments: Vec::new(),
            structured_content: Some(
                serde_json::to_value(structured_output)
                    .expect("request_user_input structured output"),
            ),
            continuation: None,
            metadata: Some(serde_json::json!({
                "question_count": request.questions.len(),
                "answers": answers.answers,
            })),
            is_error: false,
        })
    }
}

fn validate_questions(questions: &[UserInputQuestion]) -> Result<()> {
    if questions.is_empty() {
        return Err(ToolError::invalid(
            "request_user_input requires at least one question",
        ));
    }

    for question in questions {
        if !is_snake_case(&question.id) {
            return Err(ToolError::invalid(format!(
                "request_user_input question id `{}` must be snake_case",
                question.id
            )));
        }
        if question.header.trim().is_empty() {
            return Err(ToolError::invalid(
                "request_user_input question headers cannot be empty",
            ));
        }
        if question.question.trim().is_empty() {
            return Err(ToolError::invalid(
                "request_user_input question text cannot be empty",
            ));
        }
        if !(2..=3).contains(&question.options.len()) {
            return Err(ToolError::invalid(
                "request_user_input requires 2-3 options per question",
            ));
        }
        for option in &question.options {
            if option.label.trim().is_empty() || option.description.trim().is_empty() {
                return Err(ToolError::invalid(
                    "request_user_input options require non-empty label and description",
                ));
            }
        }
    }

    Ok(())
}

fn is_snake_case(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

fn format_answers(request: &UserInputRequest, response: &UserInputResponse) -> String {
    let formatted = request
        .questions
        .iter()
        .map(|question| {
            let answer = response
                .answers
                .get(&question.id)
                .map(format_answer)
                .unwrap_or_else(|| "Unanswered".to_string());
            format!("\"{}\"=\"{}\"", question.question, answer)
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "User has answered your questions: {formatted}. You can now continue with the user's answers in mind."
    )
}

fn format_answer(answer: &crate::UserInputAnswer) -> String {
    if answer.answers.is_empty() {
        return "Unanswered".to_string();
    }

    let mut parts = Vec::new();
    for entry in &answer.answers {
        if let Some(note) = entry.strip_prefix("user_note:") {
            parts.push(format!("note={}", note.trim()));
        } else {
            parts.push(entry.clone());
        }
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::RequestUserInputTool;
    use crate::{
        Tool, ToolExecutionContext, UserInputAnswer, UserInputHandler, UserInputRequest,
        UserInputResponse,
    };
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use types::ToolCallId;

    struct StaticUserInputHandler {
        response: UserInputResponse,
    }

    #[async_trait]
    impl UserInputHandler for StaticUserInputHandler {
        async fn request_input(
            &self,
            _request: UserInputRequest,
        ) -> crate::Result<UserInputResponse> {
            Ok(self.response.clone())
        }
    }

    #[tokio::test]
    async fn request_user_input_returns_structured_answers() {
        let tool = RequestUserInputTool::new();
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "questions": [{
                        "id": "scope_choice",
                        "header": "Scope",
                        "question": "Which scope should I target?",
                        "options": [
                            {
                                "label": "Runtime (Recommended)",
                                "description": "Touches only substrate runtime code."
                            },
                            {
                                "label": "Host",
                                "description": "Touches the code-agent app."
                            }
                        ]
                    }]
                }),
                &ToolExecutionContext {
                    user_input_handler: Some(Arc::new(StaticUserInputHandler {
                        response: UserInputResponse {
                            answers: BTreeMap::from([(
                                "scope_choice".to_string(),
                                UserInputAnswer {
                                    answers: vec!["Runtime (Recommended)".to_string()],
                                },
                            )]),
                        },
                    })),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(result.structured_content.unwrap()["question_count"], 1);
        assert!(result.text_content().contains("Runtime (Recommended)"));
    }

    #[tokio::test]
    async fn request_user_input_rejects_invalid_question_shape() {
        let tool = RequestUserInputTool::new();
        let error = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "questions": [{
                        "id": "NotSnake",
                        "header": "TooLongForHeader",
                        "question": "",
                        "options": []
                    }]
                }),
                &ToolExecutionContext::default(),
            )
            .await
            .expect_err("invalid request_user_input payload should fail");
        assert!(error.to_string().contains("snake_case"));
    }

    #[tokio::test]
    async fn request_user_input_requires_host_handler() {
        let tool = RequestUserInputTool::new();
        let error = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "questions": [{
                        "id": "scope_choice",
                        "header": "Scope",
                        "question": "Which scope should I target?",
                        "options": [
                            {
                                "label": "Runtime (Recommended)",
                                "description": "Touches only substrate runtime code."
                            },
                            {
                                "label": "Host",
                                "description": "Touches the code-agent app."
                            }
                        ]
                    }]
                }),
                &ToolExecutionContext::default(),
            )
            .await
            .expect_err("missing host handler should fail");
        assert!(error.to_string().contains("host user-input handler"));
    }

    #[tokio::test]
    async fn request_user_input_accepts_more_than_three_questions() {
        let tool = RequestUserInputTool::new();
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "questions": [
                        {
                            "id": "scope_choice",
                            "header": "Scope",
                            "question": "Which scope should I target?",
                            "options": [
                                {
                                    "label": "Runtime",
                                    "description": "Touches substrate code."
                                },
                                {
                                    "label": "Host",
                                    "description": "Touches app code."
                                }
                            ]
                        },
                        {
                            "id": "risk_choice",
                            "header": "Risk",
                            "question": "Should I keep the change narrow?",
                            "options": [
                                {
                                    "label": "Yes",
                                    "description": "Avoid broader cleanup."
                                },
                                {
                                    "label": "No",
                                    "description": "Broader cleanup is acceptable."
                                }
                            ]
                        },
                        {
                            "id": "test_choice",
                            "header": "Tests",
                            "question": "Should I add tests now?",
                            "options": [
                                {
                                    "label": "Yes",
                                    "description": "Add validation in this pass."
                                },
                                {
                                    "label": "Later",
                                    "description": "Defer tests for a follow-up."
                                }
                            ]
                        },
                        {
                            "id": "docs_choice",
                            "header": "Docs",
                            "question": "Should I update docs in the same pass?",
                            "options": [
                                {
                                    "label": "Yes",
                                    "description": "Update docs alongside the code."
                                },
                                {
                                    "label": "No",
                                    "description": "Leave docs for later."
                                }
                            ]
                        }
                    ]
                }),
                &ToolExecutionContext {
                    user_input_handler: Some(Arc::new(StaticUserInputHandler {
                        response: UserInputResponse {
                            answers: BTreeMap::from([
                                (
                                    "scope_choice".to_string(),
                                    UserInputAnswer {
                                        answers: vec!["Runtime".to_string()],
                                    },
                                ),
                                (
                                    "risk_choice".to_string(),
                                    UserInputAnswer {
                                        answers: vec!["Yes".to_string()],
                                    },
                                ),
                                (
                                    "test_choice".to_string(),
                                    UserInputAnswer {
                                        answers: vec!["Yes".to_string()],
                                    },
                                ),
                                (
                                    "docs_choice".to_string(),
                                    UserInputAnswer {
                                        answers: vec!["Yes".to_string()],
                                    },
                                ),
                            ]),
                        },
                    })),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(result.structured_content.unwrap()["question_count"], 4);
    }
}
