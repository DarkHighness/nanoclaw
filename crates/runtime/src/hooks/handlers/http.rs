use crate::{Result, RuntimeError};
use async_trait::async_trait;
use reqwest::Method;
use types::{
    HookContext, HookExecutionPolicy, HookHandler, HookNetworkPolicy, HookRegistration, HookResult,
};

#[async_trait]
pub trait HttpHookExecutor: Send + Sync {
    async fn execute(
        &self,
        registration: &HookRegistration,
        context: HookContext,
    ) -> Result<HookResult>;
}

pub struct ReqwestHttpHookExecutor {
    client: reqwest::Client,
}

impl Default for ReqwestHttpHookExecutor {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
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
        ensure_http_allowed(&registration.execution, &http.url)?;
        let mut request = self.client.request(
            Method::from_bytes(http.method.as_bytes()).map_err(|error| {
                RuntimeError::hook(format!("invalid hook HTTP method: {error}"))
            })?,
            &http.url,
        );
        for (key, value) in &http.headers {
            request = request.header(key, value);
        }
        let response = request.json(&context).send().await?.error_for_status()?;
        Ok(response.json::<HookResult>().await?)
    }
}

fn ensure_http_allowed(execution: &Option<HookExecutionPolicy>, url: &str) -> Result<()> {
    let Some(execution) = execution else {
        return Ok(());
    };
    match &execution.network {
        HookNetworkPolicy::Deny => Err(RuntimeError::hook(format!(
            "hook network access denied for url `{url}`"
        ))),
        HookNetworkPolicy::Allow => Ok(()),
        HookNetworkPolicy::AllowDomains { domains } => {
            let host = reqwest::Url::parse(url)
                .map_err(|error| RuntimeError::hook(format!("invalid hook HTTP url: {error}")))?
                .host_str()
                .ok_or_else(|| RuntimeError::hook("hook HTTP url missing host"))?
                .to_string();
            if domains.iter().any(|domain| domain == &host) {
                Ok(())
            } else {
                Err(RuntimeError::hook(format!(
                    "hook HTTP url `{url}` is outside granted domains"
                )))
            }
        }
    }
}
