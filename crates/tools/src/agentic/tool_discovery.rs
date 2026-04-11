use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::registry::{Tool, ToolRegistry};
use crate::{Result, ToolError, ToolExecutionContext};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cmp::Reverse;
use std::collections::BTreeSet;
use types::{
    CallId, MessagePart, ToolCallId, ToolName, ToolOutputMode, ToolResult, ToolSource, ToolSpec,
    ToolVisibilityContext,
};

const DEFAULT_DISCOVERY_LIMIT: usize = 8;
const MAX_DISCOVERY_LIMIT: usize = 20;
const DISCOVERY_TOOL_NAMES: &[&str] = &["tool_search", "tool_suggest"];

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct ToolSearchInput {
    pub query: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct ToolSuggestInput {
    pub query: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct ToolDiscoveryItem {
    name: ToolName,
    description: String,
    kind: String,
    source: String,
    aliases: Vec<String>,
    supports_parallel_tool_calls: bool,
    reason: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct ToolDiscoveryOutput {
    query: String,
    matches: Vec<ToolDiscoveryItem>,
}

#[derive(Clone)]
pub struct ToolSearchTool {
    // Keep a live registry handle so discovery results include tools registered
    // after boot, such as MCP projections, plugins, and workspace custom tools.
    registry: ToolRegistry,
}

#[derive(Clone)]
pub struct ToolSuggestTool {
    // Suggestions need the same shared-state view as tool_search so they stay
    // aligned with runtime-visible tool surfaces instead of a boot-time snapshot.
    registry: ToolRegistry,
}

impl ToolSearchTool {
    #[must_use]
    pub fn new(registry: ToolRegistry) -> Self {
        Self { registry }
    }
}

impl ToolSuggestTool {
    #[must_use]
    pub fn new(registry: ToolRegistry) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for ToolSearchTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "tool_search",
            "Search available model-visible tools by name, alias, and description.",
            serde_json::to_value(schema_for!(ToolSearchInput)).expect("tool_search schema"),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(ToolDiscoveryOutput))
                .expect("tool_search output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: ToolSearchInput = serde_json::from_value(arguments)?;
        let matches = search_registry(
            &self.registry,
            &input.query,
            input.limit,
            &ctx.model_visibility,
        )?;
        build_discovery_result(call_id, "tool_search", input.query, matches)
    }
}

#[async_trait]
impl Tool for ToolSuggestTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "tool_suggest",
            "Suggest the most relevant available tools for a task or intent description.",
            serde_json::to_value(schema_for!(ToolSuggestInput)).expect("tool_suggest schema"),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(ToolDiscoveryOutput))
                .expect("tool_suggest output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: ToolSuggestInput = serde_json::from_value(arguments)?;
        let matches = search_registry(
            &self.registry,
            &input.query,
            input.limit,
            &ctx.model_visibility,
        )?;
        build_discovery_result(call_id, "tool_suggest", input.query, matches)
    }
}

fn search_registry(
    registry: &ToolRegistry,
    query: &str,
    limit: Option<usize>,
    visibility: &ToolVisibilityContext,
) -> Result<Vec<ToolDiscoveryItem>> {
    let normalized_query = query.trim().to_lowercase();
    if normalized_query.is_empty() {
        return Err(ToolError::invalid("tool discovery query must not be empty"));
    }
    let limit = limit
        .unwrap_or(DEFAULT_DISCOVERY_LIMIT)
        .clamp(1, MAX_DISCOVERY_LIMIT);
    let query_tokens = tokenize(&normalized_query);

    let mut ranked = registry
        .specs()
        .into_iter()
        .filter(|spec| is_discoverable(spec, visibility))
        .filter_map(|spec| {
            let match_result = rank_spec(&spec, &normalized_query, &query_tokens)?;
            Some((match_result, spec))
        })
        .collect::<Vec<_>>();

    ranked.sort_by_key(|(match_result, spec)| {
        (
            Reverse(match_result.score),
            Reverse(match_result.name_hits),
            spec.name.clone(),
        )
    });

    Ok(ranked
        .into_iter()
        .take(limit)
        .map(|(match_result, spec)| ToolDiscoveryItem {
            name: spec.name,
            description: spec.description,
            kind: format!("{:?}", spec.kind).to_lowercase(),
            source: tool_source_label(&spec.source),
            aliases: spec
                .aliases
                .into_iter()
                .map(|alias| alias.into_inner())
                .collect(),
            supports_parallel_tool_calls: spec.supports_parallel_tool_calls,
            reason: match_result.reason,
        })
        .collect())
}

fn tool_source_label(source: &ToolSource) -> String {
    match source {
        ToolSource::Builtin => "builtin".to_string(),
        ToolSource::Dynamic => "dynamic".to_string(),
        ToolSource::Plugin { plugin } => format!("plugin:{plugin}"),
        ToolSource::McpTool { server_name } => format!("mcp_tool:{server_name}"),
        ToolSource::McpResource { server_name } => format!("mcp_resource:{server_name}"),
        ToolSource::ProviderBuiltin { provider } => format!("provider:{provider}"),
    }
}

fn is_discoverable(spec: &ToolSpec, visibility: &ToolVisibilityContext) -> bool {
    !DISCOVERY_TOOL_NAMES.contains(&spec.name.as_str()) && spec.is_model_visible(visibility)
}

#[derive(Clone, Debug)]
struct MatchResult {
    score: usize,
    name_hits: usize,
    reason: String,
}

fn rank_spec(spec: &ToolSpec, query: &str, query_tokens: &[String]) -> Option<MatchResult> {
    let name = spec.name.as_str().to_lowercase();
    let description = spec.description.to_lowercase();
    let aliases = spec
        .aliases
        .iter()
        .map(|alias| alias.as_str().to_lowercase())
        .collect::<Vec<_>>();

    let mut score = 0;
    let mut name_hits = 0;
    let mut reasons = Vec::new();

    if name == query {
        score += 1_000;
        name_hits += 1;
        reasons.push("exact name match".to_string());
    } else if name.contains(query) {
        score += 400;
        name_hits += 1;
        reasons.push("name contains query".to_string());
    }

    if aliases.iter().any(|alias| alias == query) {
        score += 700;
        name_hits += 1;
        reasons.push("exact alias match".to_string());
    } else if aliases.iter().any(|alias| alias.contains(query)) {
        score += 280;
        reasons.push("alias contains query".to_string());
    }

    let matched_name_tokens = query_tokens
        .iter()
        .filter(|token| name.contains(token.as_str()))
        .cloned()
        .collect::<BTreeSet<_>>();
    if !matched_name_tokens.is_empty() {
        score += matched_name_tokens.len() * 80;
        name_hits += matched_name_tokens.len();
        reasons.push(format!(
            "name matched {}",
            matched_name_tokens
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let matched_alias_tokens = query_tokens
        .iter()
        .filter(|token| aliases.iter().any(|alias| alias.contains(token.as_str())))
        .cloned()
        .collect::<BTreeSet<_>>();
    if !matched_alias_tokens.is_empty() {
        score += matched_alias_tokens.len() * 40;
        reasons.push(format!(
            "alias matched {}",
            matched_alias_tokens
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let matched_description_tokens = query_tokens
        .iter()
        .filter(|token| description.contains(token.as_str()))
        .cloned()
        .collect::<BTreeSet<_>>();
    if !matched_description_tokens.is_empty() {
        score += matched_description_tokens.len() * 18;
        reasons.push(format!(
            "description matched {}",
            matched_description_tokens
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    (score > 0).then(|| MatchResult {
        score,
        name_hits,
        reason: reasons.into_iter().take(2).collect::<Vec<_>>().join("; "),
    })
}

fn tokenize(query: &str) -> Vec<String> {
    query
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
        .map(str::trim)
        .filter(|token| token.len() >= 2)
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn build_discovery_result(
    call_id: ToolCallId,
    tool_name: &str,
    query: String,
    matches: Vec<ToolDiscoveryItem>,
) -> Result<ToolResult> {
    let output = ToolDiscoveryOutput {
        query: query.clone(),
        matches,
    };
    let structured = serde_json::to_value(&output)
        .map_err(|error| ToolError::invalid_state(error.to_string()))?;
    Ok(ToolResult {
        id: call_id.clone(),
        call_id: CallId::from(&call_id),
        tool_name: ToolName::from(tool_name),
        parts: vec![MessagePart::text(render_discovery_text(tool_name, &output))],
        attachments: Vec::new(),
        structured_content: Some(structured.clone()),
        continuation: None,
        metadata: Some(structured),
        is_error: false,
    })
}

fn render_discovery_text(tool_name: &str, output: &ToolDiscoveryOutput) -> String {
    if output.matches.is_empty() {
        return format!("{tool_name} query=\"{}\"\nNo matching tools.", output.query);
    }

    let mut lines = vec![format!(
        "{tool_name} query=\"{}\" matches={}",
        output.query,
        output.matches.len()
    )];
    lines.extend(output.matches.iter().map(|entry| {
        format!(
            "tool {} — {} ({})",
            entry.name, entry.description, entry.reason
        )
    }));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{ToolSearchTool, ToolSuggestTool};
    use crate::{Result, Tool, ToolExecutionContext, ToolRegistry};
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use types::{
        ToolAvailability, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSource, ToolSpec,
        ToolVisibilityContext,
    };

    #[derive(Clone)]
    struct FakeTool {
        name: &'static str,
        description: &'static str,
        hidden_from_model: bool,
        provider_allowlist: Vec<String>,
        model_allowlist: Vec<String>,
        role_allowlist: Vec<String>,
    }

    #[async_trait]
    impl Tool for FakeTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec::function(
                self.name,
                self.description,
                json!({"type":"object","properties":{}}),
                ToolOutputMode::Text,
                ToolOrigin::Local,
                ToolSource::Builtin,
            )
            .with_availability(ToolAvailability {
                hidden_from_model: self.hidden_from_model,
                provider_allowlist: self.provider_allowlist.clone(),
                model_allowlist: self.model_allowlist.clone(),
                role_allowlist: self.role_allowlist.clone(),
                ..ToolAvailability::default()
            })
        }

        async fn execute(
            &self,
            call_id: ToolCallId,
            _arguments: Value,
            _ctx: &ToolExecutionContext,
        ) -> Result<ToolResult> {
            Ok(ToolResult::text(call_id, self.name, self.description))
        }
    }

    #[tokio::test]
    async fn tool_search_prefers_exact_name_matches() {
        let mut registry = ToolRegistry::new();
        let discovery_registry = registry.clone();
        registry.register(ToolSearchTool::new(discovery_registry.clone()));
        registry.register(ToolSuggestTool::new(discovery_registry));
        registry.register(FakeTool {
            name: "read",
            description: "Read a file window with line numbers.",
            hidden_from_model: false,
            provider_allowlist: Vec::new(),
            model_allowlist: Vec::new(),
            role_allowlist: Vec::new(),
        });
        registry.register(FakeTool {
            name: "read_notes",
            description: "Read stored notes.",
            hidden_from_model: false,
            provider_allowlist: Vec::new(),
            model_allowlist: Vec::new(),
            role_allowlist: Vec::new(),
        });

        let tool = registry
            .get("tool_search")
            .expect("tool should be registered");
        let result = tool
            .execute(
                ToolCallId::from("call_1"),
                json!({"query": "read"}),
                &ToolExecutionContext::default(),
            )
            .await
            .expect("search should succeed");
        let structured = result.structured_content.expect("structured output");
        let matches = structured["matches"].as_array().expect("matches array");

        assert_eq!(matches[0]["name"], "read");
        assert!(matches.iter().all(|entry| entry["name"] != "tool_search"));
    }

    #[tokio::test]
    async fn tool_suggest_uses_live_registry_state_after_registration() {
        let mut registry = ToolRegistry::new();
        let discovery_registry = registry.clone();
        registry.register(ToolSuggestTool::new(discovery_registry));
        registry.register(FakeTool {
            name: "read",
            description: "Read a file window with line numbers.",
            hidden_from_model: false,
            provider_allowlist: Vec::new(),
            model_allowlist: Vec::new(),
            role_allowlist: Vec::new(),
        });
        registry.register(FakeTool {
            name: "tool_search_hidden",
            description: "Hidden helper.",
            hidden_from_model: true,
            provider_allowlist: Vec::new(),
            model_allowlist: Vec::new(),
            role_allowlist: Vec::new(),
        });
        registry.register(FakeTool {
            name: "code_symbol_search",
            description: "Search symbols and definitions across the workspace.",
            hidden_from_model: false,
            provider_allowlist: Vec::new(),
            model_allowlist: Vec::new(),
            role_allowlist: Vec::new(),
        });

        let tool = registry
            .get("tool_suggest")
            .expect("tool should be registered");
        let result = tool
            .execute(
                ToolCallId::from("call_2"),
                json!({"query": "find symbols in workspace"}),
                &ToolExecutionContext::default(),
            )
            .await
            .expect("suggest should succeed");
        let structured = result.structured_content.expect("structured output");
        let matches = structured["matches"].as_array().expect("matches array");

        assert_eq!(matches[0]["name"], "code_symbol_search");
        assert!(
            matches
                .iter()
                .all(|entry| entry["name"] != "tool_search_hidden")
        );
    }

    #[tokio::test]
    async fn tool_discovery_rejects_empty_queries() {
        let mut registry = ToolRegistry::new();
        registry.register(ToolSearchTool::new(registry.clone()));
        let tool = registry
            .get("tool_search")
            .expect("tool should be registered");

        let error = tool
            .execute(
                ToolCallId::from("call_3"),
                json!({"query": "   "}),
                &ToolExecutionContext::default(),
            )
            .await
            .expect_err("empty discovery query should fail");

        assert!(error.to_string().contains("must not be empty"));
    }

    #[tokio::test]
    async fn tool_discovery_filters_results_by_model_visibility_context() {
        let mut registry = ToolRegistry::new();
        let discovery_registry = registry.clone();
        registry.register(ToolSearchTool::new(discovery_registry));
        registry.register(FakeTool {
            name: "apply_patch",
            description: "Apply one multi-file patch.",
            hidden_from_model: false,
            provider_allowlist: vec!["openai".to_string()],
            model_allowlist: vec!["gpt-5*".to_string()],
            role_allowlist: Vec::new(),
        });
        registry.register(FakeTool {
            name: "task_batch",
            description: "Run one worker batch.",
            hidden_from_model: false,
            provider_allowlist: Vec::new(),
            model_allowlist: Vec::new(),
            role_allowlist: vec!["worker".to_string()],
        });

        let tool = registry
            .get("tool_search")
            .expect("tool should be registered");

        let root_result = tool
            .execute(
                ToolCallId::from("call_root"),
                json!({"query": "patch worker"}),
                &ToolExecutionContext {
                    model_visibility: ToolVisibilityContext::default()
                        .with_provider("openai")
                        .with_model("gpt-4.1-mini"),
                    ..ToolExecutionContext::default()
                },
            )
            .await
            .expect("root search should succeed");
        let root_matches = root_result.structured_content.expect("structured output")["matches"]
            .as_array()
            .expect("matches array")
            .clone();
        assert!(root_matches.is_empty());

        let worker_result = tool
            .execute(
                ToolCallId::from("call_worker"),
                json!({"query": "patch worker"}),
                &ToolExecutionContext {
                    model_visibility: ToolVisibilityContext::default()
                        .with_provider("openai")
                        .with_model("gpt-5.4")
                        .with_role("worker"),
                    ..ToolExecutionContext::default()
                },
            )
            .await
            .expect("worker search should succeed");
        let worker_matches =
            worker_result.structured_content.expect("structured output")["matches"]
                .as_array()
                .expect("matches array")
                .clone();

        assert!(
            worker_matches
                .iter()
                .any(|entry| entry["name"] == "apply_patch")
        );
        assert!(
            worker_matches
                .iter()
                .any(|entry| entry["name"] == "task_batch")
        );
    }
}
