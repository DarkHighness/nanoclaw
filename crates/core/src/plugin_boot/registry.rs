use anyhow::{Result, bail};
use inference::LlmServiceConfig;
use mcp::McpServerConfig;
use memory::MemoryBackend;
use plugins::PluginExecutableActivation;
use std::collections::BTreeMap;
use std::sync::Arc;
use store::SessionStore;
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
    pub primary_memory_backend: Option<Arc<dyn MemoryBackend>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriverHostMessageLevel {
    Warning,
    Diagnostic,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriverHostMessage {
    pub level: DriverHostMessageLevel,
    pub message: String,
}

impl DriverActivationOutcome {
    pub fn extend_runtime_contributions(
        &self,
        hooks: &mut Vec<HookRegistration>,
        mcp_servers: &mut Vec<McpServerConfig>,
        instructions: &mut Vec<String>,
    ) {
        self.extend_host_inputs(hooks, mcp_servers, instructions);
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.warnings.is_empty()
            && self.hooks.is_empty()
            && self.mcp_servers.is_empty()
            && self.instructions.is_empty()
            && self.diagnostics.is_empty()
    }

    pub fn host_messages(&self) -> impl Iterator<Item = DriverHostMessage> + '_ {
        self.warnings
            .iter()
            .cloned()
            .map(|message| DriverHostMessage {
                level: DriverHostMessageLevel::Warning,
                message,
            })
            .chain(
                self.diagnostics
                    .iter()
                    .cloned()
                    .map(|message| DriverHostMessage {
                        level: DriverHostMessageLevel::Diagnostic,
                        message,
                    }),
            )
    }

    pub fn remember_primary_memory_backend(&mut self, backend: Arc<dyn MemoryBackend>) {
        if self.primary_memory_backend.is_none() {
            self.primary_memory_backend = Some(backend);
        }
    }
}

pub struct PluginDriverContext<'a> {
    pub workspace_root: &'a std::path::Path,
    pub env_map: &'a agent_env::EnvMap,
    pub session_store: Option<Arc<dyn SessionStore>>,
    pub memory_reasoning_service: Option<&'a LlmServiceConfig>,
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
            let Some(factory) = self.factories.get(activation.runtime.driver.as_str()) else {
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

impl DriverActivationOutcome {
    /// Host boot code consumes declarative plugin plan output and runtime driver
    /// output through the same build pipeline. Keep the merge step centralized so
    /// every host app applies hooks, MCP servers, and instructions in the same
    /// order instead of hand-rolling partial merges.
    pub fn extend_host_inputs(
        &self,
        hooks: &mut Vec<HookRegistration>,
        mcp_servers: &mut Vec<McpServerConfig>,
        instructions: &mut Vec<String>,
    ) {
        hooks.extend(self.hooks.iter().cloned());
        mcp_servers.extend(self.mcp_servers.iter().cloned());
        instructions.extend(self.instructions.iter().cloned());
    }
}

#[cfg(test)]
mod tests {
    use super::{DriverActivationOutcome, DriverHostMessage, DriverHostMessageLevel};
    use mcp::{McpServerConfig, McpTransportConfig};
    use std::collections::BTreeMap;
    use types::{HookEvent, HookHandler, HookRegistration, HttpHookHandler};

    #[test]
    fn driver_outcome_extends_all_host_inputs() {
        let outcome = DriverActivationOutcome {
            warnings: Vec::new(),
            hooks: vec![HookRegistration {
                name: "driver-hook".into(),
                event: HookEvent::SessionStart,
                matcher: None,
                handler: HookHandler::Http(HttpHookHandler {
                    url: "https://example.test/hook".to_string(),
                    method: "POST".to_string(),
                    headers: BTreeMap::new(),
                }),
                timeout_ms: Some(500),
                execution: None,
            }],
            mcp_servers: vec![McpServerConfig {
                name: "driver-mcp".into(),
                enabled: true,
                transport: McpTransportConfig::StreamableHttp {
                    url: "https://example.test/mcp".to_string(),
                    headers: BTreeMap::new(),
                },
            }],
            instructions: vec!["driver instruction".to_string()],
            diagnostics: Vec::new(),
            primary_memory_backend: None,
        };
        let mut hooks = vec![HookRegistration {
            name: "existing-hook".into(),
            event: HookEvent::Stop,
            matcher: None,
            handler: HookHandler::Http(HttpHookHandler {
                url: "https://example.test/existing".to_string(),
                method: "POST".to_string(),
                headers: BTreeMap::new(),
            }),
            timeout_ms: None,
            execution: None,
        }];
        let mut mcp_servers = vec![McpServerConfig {
            name: "existing-mcp".into(),
            enabled: true,
            transport: McpTransportConfig::Stdio {
                command: "stdio-server".to_string(),
                args: Vec::new(),
                env: BTreeMap::new(),
                cwd: None,
            },
        }];
        let mut instructions = vec!["existing instruction".to_string()];

        outcome.extend_host_inputs(&mut hooks, &mut mcp_servers, &mut instructions);

        assert_eq!(
            hooks
                .iter()
                .map(|hook| hook.name.as_str())
                .collect::<Vec<_>>(),
            vec!["existing-hook", "driver-hook"]
        );
        assert_eq!(
            mcp_servers
                .iter()
                .map(|server| server.name.as_str())
                .collect::<Vec<_>>(),
            vec!["existing-mcp", "driver-mcp"]
        );
        assert_eq!(
            instructions,
            vec![
                "existing instruction".to_string(),
                "driver instruction".to_string()
            ]
        );
    }

    #[test]
    fn driver_outcome_emits_host_messages_in_order() {
        let outcome = DriverActivationOutcome {
            warnings: vec!["first warning".to_string(), "second warning".to_string()],
            hooks: Vec::new(),
            mcp_servers: Vec::new(),
            instructions: Vec::new(),
            diagnostics: vec!["first diagnostic".to_string()],
            primary_memory_backend: None,
        };

        assert_eq!(
            outcome.host_messages().collect::<Vec<_>>(),
            vec![
                DriverHostMessage {
                    level: DriverHostMessageLevel::Warning,
                    message: "first warning".to_string(),
                },
                DriverHostMessage {
                    level: DriverHostMessageLevel::Warning,
                    message: "second warning".to_string(),
                },
                DriverHostMessage {
                    level: DriverHostMessageLevel::Diagnostic,
                    message: "first diagnostic".to_string(),
                },
            ]
        );
    }
}
