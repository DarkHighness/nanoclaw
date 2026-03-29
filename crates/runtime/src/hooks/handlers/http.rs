use super::execution::{
    HookAuditAction, HookExecutionObserver, TracingHookExecutionObserver, authorize_network_url,
    record_completion, record_failure,
};
use crate::{Result, RuntimeError};
use async_trait::async_trait;
use reqwest::Method;
use std::sync::Arc;
use types::{HookContext, HookHandler, HookRegistration, HookResult};

#[async_trait]
pub trait HttpHookExecutor: Send + Sync {
    async fn execute(
        &self,
        registration: &HookRegistration,
        context: HookContext,
    ) -> Result<HookResult>;
}

#[derive(Clone)]
pub struct ReqwestHttpHookExecutor {
    client: reqwest::Client,
    observer: Arc<dyn HookExecutionObserver>,
}

impl Default for ReqwestHttpHookExecutor {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
            observer: Arc::new(TracingHookExecutionObserver),
        }
    }
}

impl ReqwestHttpHookExecutor {
    #[cfg(test)]
    fn with_client_and_observer(
        client: reqwest::Client,
        observer: Arc<dyn HookExecutionObserver>,
    ) -> Self {
        Self { client, observer }
    }
}

#[async_trait]
impl HttpHookExecutor for ReqwestHttpHookExecutor {
    async fn execute(
        &self,
        registration: &HookRegistration,
        context: HookContext,
    ) -> Result<HookResult> {
        let HookHandler::Http(http) = &registration.handler else {
            return Err(RuntimeError::hook(format!(
                "hook `{}` is not an HTTP hook",
                registration.name
            )));
        };
        let _authorized =
            authorize_network_url(registration, "http", &http.url, self.observer.as_ref())?;
        let mut request = self.client.request(
            Method::from_bytes(http.method.as_bytes()).map_err(|error| {
                let error = RuntimeError::hook(format!("invalid hook HTTP method: {error}"));
                record_failure(
                    self.observer.as_ref(),
                    registration,
                    "http",
                    HookAuditAction::NetworkRequest,
                    http.url.clone(),
                    &error,
                );
                error
            })?,
            &http.url,
        );
        for (key, value) in &http.headers {
            request = request.header(key, value);
        }
        let response = request
            .json(&context)
            .send()
            .await
            .map_err(|error| {
                let error = RuntimeError::hook(error.to_string());
                record_failure(
                    self.observer.as_ref(),
                    registration,
                    "http",
                    HookAuditAction::NetworkRequest,
                    http.url.clone(),
                    &error,
                );
                error
            })?
            .error_for_status()
            .map_err(|error| {
                let error = RuntimeError::hook(error.to_string());
                record_failure(
                    self.observer.as_ref(),
                    registration,
                    "http",
                    HookAuditAction::NetworkRequest,
                    http.url.clone(),
                    &error,
                );
                error
            })?;
        let result = response.json::<HookResult>().await.map_err(|error| {
            let error = RuntimeError::hook(error.to_string());
            record_failure(
                self.observer.as_ref(),
                registration,
                "http",
                HookAuditAction::NetworkRequest,
                http.url.clone(),
                &error,
            );
            error
        })?;
        record_completion(
            self.observer.as_ref(),
            registration,
            "http",
            HookAuditAction::NetworkRequest,
            http.url.clone(),
        );
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::{HttpHookExecutor, ReqwestHttpHookExecutor};
    use crate::hooks::handlers::execution::{
        HookAuditAction, HookAuditEvent, HookAuditOutcome, HookExecutionObserver,
    };
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use types::{
        AgentSessionId, HookContext, HookEffect, HookEvent, HookExecutionPolicy, HookHandler,
        HookNetworkPolicy, HookRegistration, HookResult, HttpHookHandler, MessagePart, MessageRole,
        RunId,
    };
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[derive(Default)]
    struct RecordingObserver {
        events: Mutex<Vec<HookAuditEvent>>,
    }

    impl HookExecutionObserver for RecordingObserver {
        fn record(&self, event: HookAuditEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    fn base_context() -> HookContext {
        HookContext {
            event: HookEvent::Notification,
            run_id: RunId::from("run_1"),
            agent_session_id: AgentSessionId::from("session_1"),
            turn_id: None,
            fields: BTreeMap::new(),
            payload: serde_json::json!({"hello":"world"}),
        }
    }

    #[tokio::test]
    async fn http_hook_uses_shared_network_audit_plane() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(200).set_body_json(HookResult {
                effects: vec![HookEffect::AppendMessage {
                    role: MessageRole::System,
                    parts: vec![MessagePart::text("ok")],
                }],
            }))
            .mount(&server)
            .await;

        let observer = Arc::new(RecordingObserver::default());
        let executor = ReqwestHttpHookExecutor::with_client_and_observer(
            reqwest::Client::new(),
            observer.clone(),
        );
        let result = executor
            .execute(
                &HookRegistration {
                    name: "http".to_string(),
                    event: HookEvent::Notification,
                    matcher: None,
                    handler: HookHandler::Http(HttpHookHandler {
                        url: format!("{}/hook", &server.uri()),
                        method: "POST".to_string(),
                        headers: BTreeMap::new(),
                    }),
                    timeout_ms: None,
                    execution: Some(HookExecutionPolicy {
                        plugin_id: Some("plugin".to_string()),
                        network: HookNetworkPolicy::AllowDomains {
                            domains: vec!["127.0.0.1".to_string(), "localhost".to_string()],
                        },
                        ..HookExecutionPolicy::default()
                    }),
                },
                base_context(),
            )
            .await
            .unwrap();

        assert_eq!(
            result.effects,
            vec![HookEffect::AppendMessage {
                role: MessageRole::System,
                parts: vec![MessagePart::text("ok")],
            }]
        );
        let events = observer.events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].action, HookAuditAction::NetworkRequest);
        assert_eq!(events[0].outcome, HookAuditOutcome::Allowed);
        assert_eq!(events[1].outcome, HookAuditOutcome::Completed);
    }

    #[tokio::test]
    async fn http_hook_denies_unauthorized_domains() {
        let observer = Arc::new(RecordingObserver::default());
        let executor = ReqwestHttpHookExecutor::with_client_and_observer(
            reqwest::Client::new(),
            observer.clone(),
        );
        let error = executor
            .execute(
                &HookRegistration {
                    name: "http".to_string(),
                    event: HookEvent::Notification,
                    matcher: None,
                    handler: HookHandler::Http(HttpHookHandler {
                        url: "https://example.com/hook".to_string(),
                        method: "POST".to_string(),
                        headers: BTreeMap::new(),
                    }),
                    timeout_ms: None,
                    execution: Some(HookExecutionPolicy {
                        plugin_id: Some("plugin".to_string()),
                        network: HookNetworkPolicy::AllowDomains {
                            domains: vec!["api.example.com".to_string()],
                        },
                        ..HookExecutionPolicy::default()
                    }),
                },
                base_context(),
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("outside granted domains"));
        let events = observer.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].outcome,
            HookAuditOutcome::Denied {
                reason: error.to_string()
            }
        );
    }
}
