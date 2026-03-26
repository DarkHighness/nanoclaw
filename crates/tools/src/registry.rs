use crate::{Result, ToolExecutionContext};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use types::{ToolCallId, ToolResult, ToolSpec};

#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult>;
}

#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<T>(&mut self, tool: T)
    where
        T: Tool + 'static,
    {
        self.register_arc(Arc::new(tool));
    }

    pub fn register_arc(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.spec().name.clone(), tool);
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    #[must_use]
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|tool| tool.spec()).collect()
    }

    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    #[must_use]
    pub fn filtered_by_names(&self, allowed_names: &[String]) -> Self {
        let allowed = allowed_names
            .iter()
            .collect::<std::collections::BTreeSet<_>>();
        let tools = self
            .tools
            .iter()
            .filter(|(name, _)| allowed.contains(name))
            .map(|(name, tool)| (name.clone(), tool.clone()))
            .collect();
        Self { tools }
    }
}

#[cfg(test)]
mod tests {
    use super::{Tool, ToolRegistry};
    use crate::{Result, ToolExecutionContext};
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use types::{ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};

    #[derive(Clone)]
    struct NamedTool(&'static str);

    #[async_trait]
    impl Tool for NamedTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: self.0.to_string(),
                description: format!("tool {}", self.0),
                input_schema: json!({"type":"object","properties":{}}),
                output_mode: ToolOutputMode::Text,
                output_schema: None,
                origin: ToolOrigin::Local,
                annotations: Default::default(),
            }
        }

        async fn execute(
            &self,
            call_id: ToolCallId,
            _arguments: Value,
            _ctx: &ToolExecutionContext,
        ) -> Result<ToolResult> {
            Ok(ToolResult::text(call_id, self.0, self.0))
        }
    }

    #[test]
    fn registry_exposes_names_and_specs_in_stable_sorted_order() {
        let mut registry = ToolRegistry::new();
        registry.register(NamedTool("write"));
        registry.register(NamedTool("bash"));
        registry.register(NamedTool("read"));

        assert_eq!(registry.names(), vec!["bash", "read", "write"]);
        assert_eq!(
            registry
                .specs()
                .into_iter()
                .map(|tool| tool.name)
                .collect::<Vec<_>>(),
            vec!["bash", "read", "write"]
        );
    }

    #[test]
    fn registry_can_be_filtered_by_name() {
        let mut registry = ToolRegistry::new();
        registry.register(NamedTool("write"));
        registry.register(NamedTool("bash"));
        registry.register(NamedTool("read"));

        let filtered = registry.filtered_by_names(&["read".to_string(), "write".to_string()]);
        assert_eq!(filtered.names(), vec!["read", "write"]);
    }
}
