use crate::{Result, RuntimeError};
use async_trait::async_trait;
use std::collections::BTreeMap;
use tokio::process::Command;
use types::{HookContext, HookOutput};

#[async_trait]
pub trait CommandHookExecutor: Send + Sync {
    async fn execute(&self, command: &str, context: HookContext) -> Result<HookOutput>;
}

#[derive(Clone, Debug, Default)]
pub struct DefaultCommandHookExecutor {
    extra_env: BTreeMap<String, String>,
}

impl DefaultCommandHookExecutor {
    #[must_use]
    pub fn new(extra_env: BTreeMap<String, String>) -> Self {
        Self { extra_env }
    }
}

#[async_trait]
impl CommandHookExecutor for DefaultCommandHookExecutor {
    async fn execute(&self, command: &str, context: HookContext) -> Result<HookOutput> {
        let mut process = Command::new("/bin/sh");
        process.arg("-lc").arg(command);
        process.envs(&self.extra_env);
        process.env(
            "AGENT_CORE_HOOK_PAYLOAD",
            serde_json::to_string(&context).unwrap_or_default(),
        );
        let output = process.output().await?;
        if !output.status.success() {
            return Err(RuntimeError::hook(format!(
                "hook command failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() {
            return Ok(HookOutput::default());
        }
        match serde_json::from_str::<HookOutput>(&stdout) {
            Ok(value) => Ok(value),
            Err(_) => Ok(HookOutput {
                system_message: Some(stdout),
                ..HookOutput::default()
            }),
        }
    }
}
