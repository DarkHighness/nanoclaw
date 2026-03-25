use crate::{Result, RuntimeError};
use agent_core_types::{HookContext, HookOutput};
use async_trait::async_trait;
use reqwest::Method;
use std::collections::HashMap;

#[async_trait]
pub trait HttpHookExecutor: Send + Sync {
    async fn execute(
        &self,
        method: &str,
        url: &str,
        headers: &HashMap<String, String>,
        context: HookContext,
    ) -> Result<HookOutput>;
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
        method: &str,
        url: &str,
        headers: &HashMap<String, String>,
        context: HookContext,
    ) -> Result<HookOutput> {
        let mut request = self.client.request(
            Method::from_bytes(method.as_bytes()).map_err(|error| {
                RuntimeError::hook(format!("invalid hook HTTP method: {error}"))
            })?,
            url,
        );
        for (key, value) in headers {
            request = request.header(key, value);
        }
        let response = request.json(&context).send().await?.error_for_status()?;
        Ok(response.json::<HookOutput>().await?)
    }
}
