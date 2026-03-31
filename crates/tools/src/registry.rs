use crate::{Result, ToolError, ToolExecutionContext};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use types::{DynamicToolSpec, ToolCallId, ToolName, ToolResult, ToolSpec};

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

type DynamicToolFuture = Pin<Box<dyn Future<Output = Result<ToolResult>> + Send>>;

pub type DynamicToolHandler =
    Arc<dyn Fn(ToolCallId, Value, ToolExecutionContext) -> DynamicToolFuture + Send + Sync>;

#[derive(Clone)]
pub struct DynamicTool {
    spec: ToolSpec,
    handler: DynamicToolHandler,
}

impl DynamicTool {
    #[must_use]
    pub fn new(spec: DynamicToolSpec, handler: DynamicToolHandler) -> Self {
        Self {
            spec: spec.into_tool_spec(),
            handler,
        }
    }

    #[must_use]
    pub fn from_tool_spec(spec: ToolSpec, handler: DynamicToolHandler) -> Self {
        Self { spec, handler }
    }
}

#[async_trait]
impl Tool for DynamicTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        (self.handler)(call_id, arguments, ctx.clone()).await
    }
}

#[derive(Clone)]
struct ToolEntry {
    spec: ToolSpec,
    tool: Arc<dyn Tool>,
}

#[derive(Clone, Default)]
struct ToolRegistryState {
    tools: BTreeMap<ToolName, ToolEntry>,
    aliases: BTreeMap<ToolName, ToolName>,
}

impl ToolRegistryState {
    fn canonical_name_for(&self, name: &str) -> Option<ToolName> {
        if self.tools.contains_key(name) {
            Some(ToolName::from(name))
        } else {
            self.aliases.get(name).cloned()
        }
    }
}

#[derive(Clone, Default)]
pub struct ToolRegistry {
    state: Arc<RwLock<ToolRegistryState>>,
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
        self.try_register_arc(tool)
            .expect("tool registration should not conflict");
    }

    pub fn try_register_arc(&self, tool: Arc<dyn Tool>) -> Result<()> {
        let spec = tool.spec();
        self.insert_entry(ToolEntry { spec, tool })
    }

    pub fn register_dynamic(
        &self,
        spec: DynamicToolSpec,
        handler: DynamicToolHandler,
    ) -> Result<()> {
        self.try_register_arc(Arc::new(DynamicTool::new(spec, handler)))
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        let state = self.state.read().expect("tool registry read lock");
        let canonical = state.canonical_name_for(name)?;
        state.tools.get(&canonical).map(|entry| entry.tool.clone())
    }

    #[must_use]
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.state
            .read()
            .expect("tool registry read lock")
            .tools
            .values()
            .map(|entry| entry.spec.clone())
            .collect()
    }

    #[must_use]
    pub fn names(&self) -> Vec<ToolName> {
        self.state
            .read()
            .expect("tool registry read lock")
            .tools
            .keys()
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn filtered_by_names(&self, allowed_names: &[ToolName]) -> Self {
        let state = self.state.read().expect("tool registry read lock");
        let allowed = allowed_names
            .iter()
            .filter_map(|name| state.canonical_name_for(name.as_str()))
            .collect::<BTreeSet<_>>();
        let tools = state
            .tools
            .iter()
            .filter(|(name, _)| allowed.contains(*name))
            .map(|(name, entry)| (name.clone(), entry.clone()))
            .collect();
        let aliases = state
            .aliases
            .iter()
            .filter(|(_, canonical)| allowed.contains(*canonical))
            .map(|(alias, canonical)| (alias.clone(), canonical.clone()))
            .collect();
        Self {
            state: Arc::new(RwLock::new(ToolRegistryState { tools, aliases })),
        }
    }

    fn insert_entry(&self, mut entry: ToolEntry) -> Result<()> {
        let mut state = self.state.write().expect("tool registry write lock");
        let name = entry.spec.name.clone();
        if state.tools.contains_key(&name) || state.aliases.contains_key(&name) {
            return Err(ToolError::invalid_state(format!(
                "tool registry already contains `{name}`"
            )));
        }

        let mut seen_aliases = BTreeSet::new();
        entry
            .spec
            .aliases
            .retain(|alias| alias != &name && seen_aliases.insert(alias.clone()));
        for alias in &entry.spec.aliases {
            if state.tools.contains_key(alias) || state.aliases.contains_key(alias) {
                return Err(ToolError::invalid_state(format!(
                    "tool registry alias `{alias}` conflicts with an existing tool"
                )));
            }
        }

        for alias in &entry.spec.aliases {
            state.aliases.insert(alias.clone(), name.clone());
        }
        state.tools.insert(name, entry);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{DynamicToolHandler, Tool, ToolRegistry};
    use crate::{Result, ToolExecutionContext};
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use std::sync::Arc;
    use types::{
        DynamicToolSpec, ToolCallId, ToolName, ToolOrigin, ToolOutputMode, ToolResult, ToolSource,
        ToolSpec,
    };

    #[derive(Clone)]
    struct NamedTool(&'static str);

    #[async_trait]
    impl Tool for NamedTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec::function(
                ToolName::from(self.0),
                format!("tool {}", self.0),
                json!({"type":"object","properties":{}}),
                ToolOutputMode::Text,
                ToolOrigin::Local,
                ToolSource::Builtin,
            )
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
        registry.register(NamedTool("exec_command"));
        registry.register(NamedTool("read"));

        assert_eq!(
            registry
                .names()
                .into_iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>(),
            vec!["exec_command", "read", "write"]
        );
        assert_eq!(
            registry
                .specs()
                .into_iter()
                .map(|tool| tool.name.to_string())
                .collect::<Vec<_>>(),
            vec!["exec_command", "read", "write"]
        );
    }

    #[test]
    fn registry_can_be_filtered_by_name() {
        let mut registry = ToolRegistry::new();
        registry.register(NamedTool("write"));
        registry.register(NamedTool("exec_command"));
        registry.register(NamedTool("read"));

        let filtered =
            registry.filtered_by_names(&[ToolName::from("read"), ToolName::from("write")]);
        assert_eq!(
            filtered
                .names()
                .into_iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>(),
            vec!["read", "write"]
        );
    }

    #[tokio::test]
    async fn dynamic_registry_entries_are_visible_through_aliases_and_shared_clones() {
        let registry = ToolRegistry::new();
        let handler: DynamicToolHandler = Arc::new(|call_id, arguments, _ctx| {
            Box::pin(async move {
                Ok(ToolResult::text(
                    call_id,
                    "dynamic_echo",
                    arguments["query"].as_str().unwrap_or("missing"),
                ))
            })
        });

        registry
            .register_dynamic(
                DynamicToolSpec::function(
                    "dynamic_echo",
                    "echoes one query field",
                    json!({
                        "type": "object",
                        "properties": {
                            "query": { "type": "string" }
                        },
                        "required": ["query"]
                    }),
                )
                .with_aliases(vec![ToolName::from("lookup")]),
                handler,
            )
            .unwrap();

        let tool = registry
            .get("lookup")
            .expect("dynamic alias should resolve");
        let result = tool
            .execute(
                ToolCallId::from("call-dynamic"),
                json!({ "query": "needle" }),
                &ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        assert_eq!(result.text_content(), "needle");
        let specs = registry.specs();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].source, ToolSource::Dynamic);
        assert_eq!(specs[0].aliases, vec![ToolName::from("lookup")]);
    }

    #[test]
    fn filtered_registries_remain_snapshots_when_parent_registry_changes() {
        let mut registry = ToolRegistry::new();
        registry.register(NamedTool("read"));
        let filtered = registry.filtered_by_names(&[ToolName::from("read")]);

        registry.register(NamedTool("write"));

        assert_eq!(
            filtered
                .names()
                .into_iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>(),
            vec!["read"]
        );
    }

    #[test]
    fn registry_rejects_alias_conflicts_for_dynamic_tools() {
        let registry = ToolRegistry::new();
        registry
            .register_dynamic(
                DynamicToolSpec::function(
                    "dynamic_echo",
                    "echoes one query field",
                    json!({"type": "object"}),
                )
                .with_aliases(vec![ToolName::from("lookup")]),
                Arc::new(|call_id, _arguments, _ctx| {
                    Box::pin(async move { Ok(ToolResult::text(call_id, "dynamic_echo", "ok")) })
                }),
            )
            .unwrap();

        let error = registry
            .register_dynamic(
                DynamicToolSpec::function("other_tool", "other", json!({"type": "object"}))
                    .with_aliases(vec![ToolName::from("lookup")]),
                Arc::new(|call_id, _arguments, _ctx| {
                    Box::pin(async move { Ok(ToolResult::text(call_id, "other_tool", "ok")) })
                }),
            )
            .expect_err("conflicting aliases should be rejected");
        assert!(error.to_string().contains("lookup"));
    }
}
