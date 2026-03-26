use crate::{
    AgentRuntimeBuilder, CompactionConfig, ConversationCompactor, HookRunner, LoopDetectionConfig,
    ModelBackend, Result, RuntimeError, ToolApprovalHandler, ToolApprovalPolicy,
};
use agent_core_skills::SkillCatalog;
use agent_core_store::RunStore;
use agent_core_tools::{
    SubagentExecutor, SubagentRequest, SubagentResult, ToolError, ToolExecutionContext,
    ToolRegistry,
};
use agent_core_types::HookRegistration;
use async_trait::async_trait;
use std::sync::Arc;

const DEFAULT_EXCLUDED_CHILD_TOOLS: &[&str] = &["task", "todo_read", "todo_write"];

#[derive(Clone)]
pub struct RuntimeSubagentExecutor {
    backend: Arc<dyn ModelBackend>,
    hook_runner: Arc<HookRunner>,
    store: Arc<dyn RunStore>,
    tool_registry: ToolRegistry,
    tool_context: ToolExecutionContext,
    tool_approval_handler: Arc<dyn ToolApprovalHandler>,
    tool_approval_policy: Arc<dyn ToolApprovalPolicy>,
    conversation_compactor: Arc<dyn ConversationCompactor>,
    compaction_config: CompactionConfig,
    loop_detection_config: LoopDetectionConfig,
    instructions: Vec<String>,
    hooks: Vec<HookRegistration>,
    skill_catalog: SkillCatalog,
}

impl RuntimeSubagentExecutor {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        backend: Arc<dyn ModelBackend>,
        hook_runner: Arc<HookRunner>,
        store: Arc<dyn RunStore>,
        tool_registry: ToolRegistry,
        tool_context: ToolExecutionContext,
        tool_approval_handler: Arc<dyn ToolApprovalHandler>,
        tool_approval_policy: Arc<dyn ToolApprovalPolicy>,
        conversation_compactor: Arc<dyn ConversationCompactor>,
        compaction_config: CompactionConfig,
        loop_detection_config: LoopDetectionConfig,
        instructions: Vec<String>,
        hooks: Vec<HookRegistration>,
        skill_catalog: SkillCatalog,
    ) -> Self {
        Self {
            backend,
            hook_runner,
            store,
            tool_registry,
            tool_context,
            tool_approval_handler,
            tool_approval_policy,
            conversation_compactor,
            compaction_config,
            loop_detection_config,
            instructions,
            hooks,
            skill_catalog,
        }
    }

    fn resolve_child_tools(
        &self,
        requested: Option<&[String]>,
    ) -> Result<(ToolRegistry, Vec<String>)> {
        let allowed_names = if let Some(requested) = requested {
            requested.to_vec()
        } else {
            self.tool_registry
                .names()
                .into_iter()
                .filter(|name| !DEFAULT_EXCLUDED_CHILD_TOOLS.contains(&name.as_str()))
                .collect()
        };
        let filtered = self.tool_registry.filtered_by_names(&allowed_names);
        let resolved_names = filtered.names();
        if requested.is_some() && resolved_names.is_empty() {
            return Err(RuntimeError::invalid_state(
                "task: no allowed tools matched the parent registry",
            ));
        }
        Ok((filtered, resolved_names))
    }
}

#[async_trait]
impl SubagentExecutor for RuntimeSubagentExecutor {
    async fn run(
        &self,
        request: SubagentRequest,
    ) -> std::result::Result<SubagentResult, ToolError> {
        let (tool_registry, resolved_tools) = self
            .resolve_child_tools(request.allowed_tools.as_deref())
            .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        let mut runtime = AgentRuntimeBuilder::new(self.backend.clone(), self.store.clone())
            .hook_runner(self.hook_runner.clone())
            .tool_registry(tool_registry)
            .tool_context(self.tool_context.clone())
            .tool_approval_handler(self.tool_approval_handler.clone())
            .tool_approval_policy(self.tool_approval_policy.clone())
            .conversation_compactor(self.conversation_compactor.clone())
            .compaction_config(self.compaction_config.clone())
            .loop_detection_config(self.loop_detection_config.clone())
            .instructions(self.instructions.clone())
            .hooks(self.hooks.clone())
            .skill_catalog(self.skill_catalog.clone())
            .build();

        if let Some(steer) = request.steer.clone() {
            runtime
                .steer(
                    steer,
                    Some(
                        request
                            .agent
                            .as_deref()
                            .map(|name| format!("task:{name}"))
                            .unwrap_or_else(|| "task".to_string()),
                    ),
                )
                .await
                .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        }

        let outcome = runtime
            .run_user_prompt(request.prompt.clone())
            .await
            .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        Ok(SubagentResult {
            run_id: runtime.run_id().0,
            session_id: runtime.session_id().0,
            agent_name: request
                .agent
                .unwrap_or_else(|| "general-purpose".to_string()),
            assistant_text: outcome.assistant_text,
            allowed_tools: resolved_tools,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::RuntimeSubagentExecutor;
    use crate::Result;
    use crate::{
        AlwaysAllowToolApprovalHandler, CompactionConfig, HookRunner, LoopDetectionConfig,
        ModelBackend, NoopConversationCompactor, NoopToolApprovalPolicy,
    };
    use agent_core_skills::SkillCatalog;
    use agent_core_store::InMemoryRunStore;
    use agent_core_tools::{
        ReadTool, SubagentExecutor, SubagentRequest, ToolExecutionContext, ToolRegistry,
    };
    use agent_core_types::{ModelEvent, ModelRequest};
    use async_trait::async_trait;
    use futures::{StreamExt, stream, stream::BoxStream};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct RecordingBackend {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
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
                    delta: "child ok".to_string(),
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

    #[tokio::test]
    async fn runtime_subagent_executor_applies_steer_and_filters_tools() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(RecordingBackend::default());
        let store = Arc::new(InMemoryRunStore::new());
        let mut registry = ToolRegistry::new();
        registry.register(ReadTool::new());

        let executor = RuntimeSubagentExecutor::new(
            backend.clone(),
            Arc::new(HookRunner::default()),
            store,
            registry,
            ToolExecutionContext {
                workspace_root: dir.path().to_path_buf(),
                workspace_only: true,
                ..Default::default()
            },
            Arc::new(AlwaysAllowToolApprovalHandler),
            Arc::new(NoopToolApprovalPolicy),
            Arc::new(NoopConversationCompactor),
            CompactionConfig::default(),
            LoopDetectionConfig::default(),
            vec!["static instruction".to_string()],
            Vec::new(),
            SkillCatalog::default(),
        );

        let result = executor
            .run(SubagentRequest {
                prompt: "inspect the repository".to_string(),
                agent: Some("explore".to_string()),
                steer: Some("focus on tests".to_string()),
                allowed_tools: Some(vec!["read".to_string()]),
            })
            .await
            .unwrap();

        assert_eq!(result.agent_name, "explore");
        assert_eq!(result.assistant_text, "child ok");
        assert_eq!(result.allowed_tools, vec!["read"]);

        let requests = backend.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].tools.len(), 1);
        assert_eq!(requests[0].tools[0].name, "read");
        assert_eq!(requests[0].instructions, vec!["static instruction"]);
        assert_eq!(requests[0].messages.len(), 2);
        assert_eq!(
            requests[0].messages[0].role,
            agent_core_types::MessageRole::System
        );
        assert_eq!(requests[0].messages[0].text_content(), "focus on tests");
        assert_eq!(
            requests[0].messages[1].role,
            agent_core_types::MessageRole::User
        );
        assert_eq!(
            requests[0].messages[1].text_content(),
            "inspect the repository"
        );
    }
}
