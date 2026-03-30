use crate::{
    AgentRuntime, AlwaysAllowToolApprovalHandler, CompactionConfig, ConversationCompactor,
    HookRunner, LoopDetectionConfig, ModelBackend, NoopConversationCompactor,
    NoopToolApprovalPolicy, PermissionGrantStore, RuntimeSession, ToolApprovalHandler,
    ToolApprovalPolicy,
};
use skills::SkillCatalog;
use std::sync::Arc;
use store::SessionStore;
use tools::{ToolExecutionContext, ToolRegistry};
use types::HookRegistration;

pub struct AgentRuntimeBuilder {
    backend: Arc<dyn ModelBackend>,
    hook_runner: Arc<HookRunner>,
    store: Arc<dyn SessionStore>,
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
    session: RuntimeSession,
    permission_grants: PermissionGrantStore,
}

impl AgentRuntimeBuilder {
    #[must_use]
    pub fn new(backend: Arc<dyn ModelBackend>, store: Arc<dyn SessionStore>) -> Self {
        Self {
            backend,
            hook_runner: Arc::new(HookRunner::default()),
            store,
            tool_registry: ToolRegistry::new(),
            tool_context: ToolExecutionContext::default(),
            tool_approval_handler: Arc::new(AlwaysAllowToolApprovalHandler),
            tool_approval_policy: Arc::new(NoopToolApprovalPolicy),
            conversation_compactor: Arc::new(NoopConversationCompactor),
            compaction_config: CompactionConfig::default(),
            loop_detection_config: LoopDetectionConfig::default(),
            instructions: Vec::new(),
            hooks: Vec::new(),
            skill_catalog: SkillCatalog::default(),
            session: RuntimeSession::default(),
            permission_grants: PermissionGrantStore::default(),
        }
    }

    #[must_use]
    pub fn hook_runner(mut self, hook_runner: Arc<HookRunner>) -> Self {
        self.hook_runner = hook_runner;
        self
    }

    #[must_use]
    pub fn tool_registry(mut self, tool_registry: ToolRegistry) -> Self {
        self.tool_registry = tool_registry;
        self
    }

    #[must_use]
    pub fn tool_context(mut self, tool_context: ToolExecutionContext) -> Self {
        self.tool_context = tool_context;
        self
    }

    #[must_use]
    pub fn tool_approval_handler(
        mut self,
        tool_approval_handler: Arc<dyn ToolApprovalHandler>,
    ) -> Self {
        self.tool_approval_handler = tool_approval_handler;
        self
    }

    #[must_use]
    pub fn tool_approval_policy(
        mut self,
        tool_approval_policy: Arc<dyn ToolApprovalPolicy>,
    ) -> Self {
        self.tool_approval_policy = tool_approval_policy;
        self
    }

    #[must_use]
    pub fn conversation_compactor(
        mut self,
        conversation_compactor: Arc<dyn ConversationCompactor>,
    ) -> Self {
        self.conversation_compactor = conversation_compactor;
        self
    }

    #[must_use]
    pub fn compaction_config(mut self, compaction_config: CompactionConfig) -> Self {
        self.compaction_config = compaction_config;
        self
    }

    #[must_use]
    pub fn loop_detection_config(mut self, loop_detection_config: LoopDetectionConfig) -> Self {
        self.loop_detection_config = loop_detection_config;
        self
    }

    #[must_use]
    pub fn instructions(mut self, instructions: Vec<String>) -> Self {
        self.instructions = instructions;
        self
    }

    #[must_use]
    pub fn hooks(mut self, hooks: Vec<HookRegistration>) -> Self {
        self.hooks = hooks;
        self
    }

    #[must_use]
    pub fn skill_catalog(mut self, skill_catalog: SkillCatalog) -> Self {
        self.skill_catalog = skill_catalog;
        self
    }

    #[must_use]
    pub fn session(mut self, session: RuntimeSession) -> Self {
        self.session = session;
        self
    }

    #[must_use]
    pub fn permission_grants(mut self, permission_grants: PermissionGrantStore) -> Self {
        self.permission_grants = permission_grants;
        self
    }

    #[must_use]
    pub fn build(self) -> AgentRuntime {
        AgentRuntime::new(
            self.backend,
            self.hook_runner,
            self.store,
            self.tool_registry,
            self.tool_context,
            self.tool_approval_handler,
            self.tool_approval_policy,
            self.conversation_compactor,
            self.compaction_config,
            self.loop_detection_config,
            self.instructions,
            self.hooks,
            self.skill_catalog,
            self.session,
            self.permission_grants,
        )
    }
}
