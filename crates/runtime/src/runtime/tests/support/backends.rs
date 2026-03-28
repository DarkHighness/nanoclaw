use crate::{ModelBackend, ModelBackendCapabilities, Result};
use async_trait::async_trait;
use futures::{StreamExt, stream, stream::BoxStream};
use std::sync::{Arc, Mutex};
use types::{
    AgentCoreError, Message, ModelEvent, ModelRequest, ProviderContinuation, ToolCall, ToolCallId,
    ToolOrigin,
};

pub(in crate::runtime::tests) struct MockBackend;

#[derive(Clone, Default)]
pub(in crate::runtime::tests) struct RecordingBackend {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl RecordingBackend {
    pub(in crate::runtime::tests) fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[derive(Clone, Default)]
pub(in crate::runtime::tests) struct ContinuingBackend {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
    fail_first_continuation: Arc<Mutex<bool>>,
}

impl ContinuingBackend {
    pub(in crate::runtime::tests) fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub(in crate::runtime::tests) fn with_failed_continuation() -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            fail_first_continuation: Arc::new(Mutex::new(true)),
        }
    }
}

#[async_trait]
impl ModelBackend for MockBackend {
    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
        let user_text = request
            .messages
            .last()
            .map(Message::text_content)
            .unwrap_or_default();
        if user_text.contains("tool")
            && !request.messages.iter().any(|message| {
                message
                    .parts
                    .iter()
                    .any(|part| matches!(part, types::MessagePart::ToolResult { .. }))
            })
        {
            let call = ToolCall {
                id: ToolCallId::new(),
                call_id: "call-read-1".into(),
                tool_name: "read".into(),
                arguments: serde_json::json!({"path":"sample.txt","line_count":1}),
                origin: ToolOrigin::Local,
            };
            Ok(stream::iter(vec![
                Ok(ModelEvent::ToolCallRequested { call }),
                Ok(ModelEvent::ResponseComplete {
                    stop_reason: Some("tool_use".to_string()),
                    message_id: None,
                    continuation: None,
                    reasoning: Vec::new(),
                }),
            ])
            .boxed())
        } else {
            Ok(stream::iter(vec![
                Ok(ModelEvent::TextDelta {
                    delta: "done".to_string(),
                }),
                Ok(ModelEvent::ResponseComplete {
                    stop_reason: Some("stop".to_string()),
                    message_id: None,
                    continuation: None,
                    reasoning: Vec::new(),
                }),
            ])
            .boxed())
        }
    }
}

#[async_trait]
impl ModelBackend for RecordingBackend {
    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
        self.requests.lock().unwrap().push(request);
        Ok(stream::iter(vec![
            Ok(ModelEvent::TextDelta {
                delta: "ok".to_string(),
            }),
            Ok(ModelEvent::ResponseComplete {
                stop_reason: Some("stop".to_string()),
                message_id: None,
                continuation: None,
                reasoning: Vec::new(),
            }),
        ])
        .boxed())
    }
}

#[async_trait]
impl ModelBackend for ContinuingBackend {
    fn capabilities(&self) -> ModelBackendCapabilities {
        ModelBackendCapabilities {
            provider_managed_history: true,
            provider_native_compaction: true,
        }
    }

    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
        self.requests.lock().unwrap().push(request.clone());
        if request.continuation.is_some() {
            let mut fail_first = self.fail_first_continuation.lock().unwrap();
            if *fail_first {
                *fail_first = false;
                return Err(AgentCoreError::ProviderContinuationLost(
                    "provider lost previous_response_id".to_string(),
                )
                .into());
            }
        }

        let response_index = self.requests.lock().unwrap().len();
        Ok(stream::iter(vec![
            Ok(ModelEvent::TextDelta {
                delta: format!("response {response_index}"),
            }),
            Ok(ModelEvent::ResponseComplete {
                stop_reason: Some("stop".to_string()),
                message_id: Some(format!("msg_{response_index}").into()),
                continuation: Some(ProviderContinuation::OpenAiResponses {
                    response_id: format!("resp_{response_index}").into(),
                }),
                reasoning: Vec::new(),
            }),
        ])
        .boxed())
    }
}
