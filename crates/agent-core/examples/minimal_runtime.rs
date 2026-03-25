use agent_core::{
    AgentRuntimeBuilder, BashTool, EditTool, GlobTool, GrepTool, HookRunner, InMemoryRunStore,
    ListTool, Message, MessageRole, ModelBackend, ModelEvent, ModelRequest, PatchTool, ReadTool,
    Skill, SkillCatalog, ToolExecutionContext, ToolRegistry, WriteTool,
};
use anyhow::Result;
use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use std::collections::BTreeMap;
use std::sync::Arc;

struct EchoBackend;

#[async_trait]
impl ModelBackend for EchoBackend {
    async fn stream_turn(
        &self,
        request: ModelRequest,
    ) -> Result<BoxStream<'static, Result<ModelEvent>>> {
        let latest_user_message = request
            .messages
            .iter()
            .rev()
            .find(|message| matches!(message.role, MessageRole::User))
            .map(Message::text_content)
            .unwrap_or_else(|| "<no user message>".to_string());

        let response = format!(
            "Echo backend received the prompt: {latest_user_message}\nRegistered tools: {}\nSystem preamble entries: {}",
            request.tools.len(),
            request.instructions.len()
        );

        Ok(Box::pin(stream::iter(vec![
            Ok(ModelEvent::TextDelta { delta: response }),
            Ok(ModelEvent::ResponseComplete {
                stop_reason: Some("completed".to_string()),
                message_id: Some("example-response-1".to_string()),
                reasoning: Vec::new(),
            }),
        ])))
    }
}

fn build_system_preamble(system_prompt: Option<&str>, skill_catalog: &SkillCatalog) -> Vec<String> {
    let mut preamble = vec![
        "You are a general-purpose software agent operating inside the current workspace."
            .to_string(),
        "Inspect repository state and use tools before guessing.".to_string(),
    ];
    if let Some(system_prompt) = system_prompt
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        preamble.push(system_prompt.to_string());
    }
    if let Some(skill_manifest) = skill_catalog.prompt_manifest() {
        preamble.push(skill_manifest);
    }
    preamble
}

fn example_skill(workspace_root: &std::path::Path) -> Skill {
    Skill {
        name: "workspace-rules".to_string(),
        description: "Repository-specific guidance surfaced as a first-class skill.".to_string(),
        aliases: vec!["repo-rules".to_string()],
        body: "Read workspace policy files before making large or destructive changes.".to_string(),
        root_dir: workspace_root.join(".skills").join("workspace-rules"),
        tags: vec!["workspace".to_string(), "policy".to_string()],
        hooks: Vec::new(),
        references: Vec::new(),
        scripts: Vec::new(),
        assets: Vec::new(),
        metadata: BTreeMap::new(),
        extension_metadata: BTreeMap::new(),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let workspace_root = std::env::current_dir()?;
    let store = Arc::new(InMemoryRunStore::new());
    let backend = Arc::new(EchoBackend);

    let skill_catalog = SkillCatalog::new(vec![example_skill(&workspace_root)]);
    let system_preamble = build_system_preamble(
        Some("Prefer append-only history and explicit tool use over hidden client heuristics."),
        &skill_catalog,
    );

    let mut tools = ToolRegistry::new();
    tools.register(ReadTool::new());
    tools.register(WriteTool::new());
    tools.register(EditTool::new());
    tools.register(PatchTool::new());
    tools.register(GlobTool::new());
    tools.register(GrepTool::new());
    tools.register(ListTool::new());
    tools.register(BashTool::new());

    let mut runtime = AgentRuntimeBuilder::new(backend, store)
        .hook_runner(Arc::new(HookRunner::default()))
        .tool_registry(tools)
        .tool_context(ToolExecutionContext {
            workspace_root,
            workspace_only: true,
            model_context_window_tokens: Some(128_000),
            worktree_root: Some(std::env::current_dir()?),
            ..Default::default()
        })
        .instructions(system_preamble)
        .skill_catalog(skill_catalog)
        .build();

    let outcome = runtime
        .run_user_prompt("Summarize this runtime setup in one paragraph.")
        .await?;

    println!("run_id={}", runtime.run_id().0);
    println!("tools={}", runtime.tool_registry_names().join(", "));
    println!("{}", outcome.assistant_text);

    Ok(())
}
