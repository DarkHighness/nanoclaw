use anyhow::{Result, bail};
use mcp::McpServerConfig;
use plugins::PluginExecutableActivation;
use std::collections::BTreeMap;
use std::sync::Arc;
use store::RunStore;
use tools::ToolRegistry;
use types::HookRegistration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownDriverPolicy {
    Error,
    Warn,
}

#[derive(Default)]
pub struct DriverActivationOutcome {
    pub warnings: Vec<String>,
    pub hooks: Vec<HookRegistration>,
    pub mcp_servers: Vec<McpServerConfig>,
    pub instructions: Vec<String>,
    pub diagnostics: Vec<String>,
}

pub struct PluginDriverContext<'a> {
    pub workspace_root: &'a std::path::Path,
    pub env_map: &'a agent_env::EnvMap,
    pub run_store: Option<Arc<dyn RunStore>>,
    pub tools: &'a mut ToolRegistry,
}

pub trait PluginDriverFactory: Send + Sync {
    fn driver_id(&self) -> &'static str;

    fn activate(
        &self,
        activation: &PluginExecutableActivation,
        context: &mut PluginDriverContext<'_>,
        outcome: &mut DriverActivationOutcome,
    ) -> Result<()>;
}

#[derive(Default)]
pub struct PluginDriverRegistry {
    factories: BTreeMap<String, Arc<dyn PluginDriverFactory>>,
}

impl PluginDriverRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, factory: Arc<dyn PluginDriverFactory>) {
        self.factories
            .insert(factory.driver_id().to_string(), factory);
    }

    pub fn activate_all(
        &self,
        activations: &[PluginExecutableActivation],
        context: &mut PluginDriverContext<'_>,
        unknown_driver_policy: UnknownDriverPolicy,
    ) -> Result<DriverActivationOutcome> {
        let mut outcome = DriverActivationOutcome::default();
        for activation in activations {
            let Some(factory) = self.factories.get(&activation.runtime.driver) else {
                match unknown_driver_policy {
                    UnknownDriverPolicy::Error => bail!(
                        "plugin `{}` references unknown driver `{}`",
                        activation.plugin_id,
                        activation.runtime.driver
                    ),
                    UnknownDriverPolicy::Warn => {
                        outcome.warnings.push(format!(
                            "plugin `{}` references unknown driver `{}`",
                            activation.plugin_id, activation.runtime.driver
                        ));
                        continue;
                    }
                }
            };
            factory.activate(activation, context, &mut outcome)?;
        }
        Ok(outcome)
    }
}
