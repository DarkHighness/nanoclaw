use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::registry::Tool;
use crate::{Result, ToolError, ToolExecutionContext};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use skills::{Skill, SkillCatalog, SkillRootKind, load_skill_roots};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use tokio::fs;
use types::{MessagePart, ToolCallId, ToolOutputMode, ToolResult, ToolSpec};

const DEFAULT_SKILL_LIMIT: usize = 20;
const MAX_SKILL_LIMIT: usize = 100;
const SKILL_MANAGE_REFRESH_NOTE: &str =
    "Skill catalog changed. Re-run skills_list or skill_view before relying on updated skills.";

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct SkillsListToolInput {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct SkillViewToolInput {
    pub name: String,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub include_instruction: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SkillManageToolInput {
    Create {
        name: String,
        description: String,
        #[serde(default)]
        aliases: Vec<String>,
        #[serde(default)]
        tags: Vec<String>,
        instruction: String,
        #[serde(default)]
        platforms: Vec<String>,
        #[serde(default)]
        requires_tools: Vec<String>,
        #[serde(default)]
        fallback_for_tools: Vec<String>,
    },
    Edit {
        name: String,
        description: String,
        #[serde(default)]
        aliases: Vec<String>,
        #[serde(default)]
        tags: Vec<String>,
        instruction: String,
        #[serde(default)]
        platforms: Vec<String>,
        #[serde(default)]
        requires_tools: Vec<String>,
        #[serde(default)]
        fallback_for_tools: Vec<String>,
    },
    Patch {
        name: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        aliases: Option<Vec<String>>,
        #[serde(default)]
        tags: Option<Vec<String>>,
        #[serde(default)]
        instruction: Option<String>,
        #[serde(default)]
        platforms: Option<Vec<String>>,
        #[serde(default)]
        requires_tools: Option<Vec<String>>,
        #[serde(default)]
        fallback_for_tools: Option<Vec<String>>,
    },
    Delete {
        name: String,
    },
    WriteFile {
        name: String,
        file_path: String,
        content: String,
    },
    RemoveFile {
        name: String,
        file_path: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub skill_path: String,
    pub slash_command: String,
    pub root_kind: String,
    pub writable: bool,
    pub platforms: Vec<String>,
    pub requires_tools: Vec<String>,
    pub fallback_for_tools: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SkillDetail {
    #[serde(flatten)]
    pub summary: SkillSummary,
    pub references: Vec<String>,
    pub scripts: Vec<String>,
    pub assets: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SkillFileView {
    pub skill_name: String,
    pub file_path: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkillViewOutput {
    Skill { query: String, skill: SkillDetail },
    File { query: String, file: SkillFileView },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SkillsListOutput {
    pub query: Option<String>,
    pub result_count: usize,
    pub skills: Vec<SkillSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SkillManageOutput {
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<SkillSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    pub note: String,
}

#[derive(Clone)]
pub struct SkillsListTool {
    catalog: SkillCatalog,
}

#[derive(Clone)]
pub struct SkillViewTool {
    catalog: SkillCatalog,
}

#[derive(Clone)]
pub struct SkillManageTool {
    catalog: SkillCatalog,
}

impl SkillsListTool {
    #[must_use]
    pub fn new(catalog: SkillCatalog) -> Self {
        Self { catalog }
    }
}

impl SkillViewTool {
    #[must_use]
    pub fn new(catalog: SkillCatalog) -> Self {
        Self { catalog }
    }
}

impl SkillManageTool {
    #[must_use]
    pub fn new(catalog: SkillCatalog) -> Self {
        Self { catalog }
    }
}

#[async_trait]
impl Tool for SkillsListTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "skills_list",
            "List available skills. Use this before invoking a skill or inspecting one in detail.",
            serde_json::to_value(schema_for!(SkillsListToolInput)).expect("skills_list schema"),
            ToolOutputMode::ContentParts,
            tool_approval_profile(true, false, true, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(SkillsListOutput)).expect("skills_list output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        _ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: SkillsListToolInput = serde_json::from_value(arguments)?;
        let limit = input
            .limit
            .unwrap_or(DEFAULT_SKILL_LIMIT)
            .clamp(1, MAX_SKILL_LIMIT);
        let skills = if let Some(query) = input
            .query
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            fuzzy_skill_matches(&self.catalog, query, limit)
        } else {
            self.catalog
                .all()
                .into_iter()
                .take(limit)
                .map(|skill| skill_summary(&skill))
                .collect()
        };
        let structured = SkillsListOutput {
            query: input.query.clone(),
            result_count: skills.len(),
            skills: skills.clone(),
        };
        Ok(ToolResult {
            id: call_id.clone(),
            call_id: types::CallId::from(&call_id),
            tool_name: "skills_list".into(),
            parts: vec![MessagePart::text(render_skill_list(
                input.query.as_deref(),
                &skills,
            ))],
            attachments: Vec::new(),
            structured_content: Some(
                serde_json::to_value(structured).expect("skills_list structured output"),
            ),
            continuation: None,
            metadata: None,
            is_error: false,
        })
    }
}

#[async_trait]
impl Tool for SkillViewTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "skill_view",
            "Inspect one skill or load one companion file from a skill package.",
            serde_json::to_value(schema_for!(SkillViewToolInput)).expect("skill_view schema"),
            ToolOutputMode::ContentParts,
            tool_approval_profile(true, false, true, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(SkillViewOutput)).expect("skill_view output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: SkillViewToolInput = serde_json::from_value(arguments)?;
        let query = input.name.trim();
        if query.is_empty() {
            return Err(ToolError::invalid(
                "skill_view requires a non-empty skill name",
            ));
        }
        let skill = self
            .catalog
            .resolve(query)
            .ok_or_else(|| ToolError::invalid(format!("unknown skill `{query}`")))?;

        let (parts, structured, metadata) = if let Some(file_path) = input
            .file_path
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let relative = normalize_relative_skill_path(file_path)?;
            let absolute_path = skill.root_dir.join(&relative);
            ctx.assert_path_read_allowed(&absolute_path)?;
            let content = fs::read_to_string(&absolute_path).await?;
            let file_view = SkillFileView {
                skill_name: skill.name.clone(),
                file_path: relative.clone(),
                content: content.clone(),
            };
            (
                vec![
                    MessagePart::text(format!(
                        "Skill File\n{}\n{}",
                        file_view.file_path, file_view.content
                    )),
                    MessagePart::reference(
                        "skill",
                        Some(skill.name.clone()),
                        Some(skill.skill_path().display().to_string()),
                        Some(skill.description.clone()),
                    ),
                ],
                SkillViewOutput::File {
                    query: query.to_string(),
                    file: file_view,
                },
                Some(serde_json::json!({
                    "skill_name": skill.name,
                    "file_path": relative,
                })),
            )
        } else {
            let detail = skill_detail(&skill, input.include_instruction);
            (
                build_detail_parts(&detail),
                SkillViewOutput::Skill {
                    query: query.to_string(),
                    skill: detail.clone(),
                },
                Some(serde_json::json!({
                    "skill_name": detail.summary.name,
                    "skill_path": detail.summary.skill_path,
                })),
            )
        };

        Ok(ToolResult {
            id: call_id.clone(),
            call_id: types::CallId::from(&call_id),
            tool_name: "skill_view".into(),
            parts,
            attachments: Vec::new(),
            structured_content: Some(
                serde_json::to_value(structured).expect("skill_view structured output"),
            ),
            continuation: None,
            metadata,
            is_error: false,
        })
    }
}

#[async_trait]
impl Tool for SkillManageTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "skill_manage",
            "Create, edit, patch, delete, or manage companion files for skills in the managed skill root.",
            serde_json::to_value(schema_for!(SkillManageToolInput)).expect("skill_manage schema"),
            ToolOutputMode::ContentParts,
            tool_approval_profile(false, true, false, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(SkillManageOutput))
                .expect("skill_manage output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let input: SkillManageToolInput = serde_json::from_value(arguments)?;
        let managed_root = self
            .catalog
            .managed_root()
            .ok_or_else(|| ToolError::invalid_state("no managed skill root is configured"))?;
        ctx.assert_path_write_allowed(&managed_root.path)?;
        fs::create_dir_all(&managed_root.path).await?;

        let output = match input {
            SkillManageToolInput::Create {
                name,
                description,
                aliases,
                tags,
                instruction,
                platforms,
                requires_tools,
                fallback_for_tools,
            } => {
                let canonical = validate_skill_name(&name)?;
                if self.catalog.resolve(&canonical).is_some() {
                    return Err(ToolError::invalid(format!(
                        "skill `{canonical}` already exists"
                    )));
                }
                let skill_dir = managed_root.path.join(&canonical);
                ctx.assert_path_write_allowed(&skill_dir)?;
                fs::create_dir_all(&skill_dir).await?;
                write_skill_manifest(
                    &skill_dir,
                    &ManagedSkillSpec {
                        name: canonical.clone(),
                        description,
                        aliases,
                        tags,
                        instruction,
                        platforms,
                        requires_tools,
                        fallback_for_tools,
                        metadata: BTreeMap::new(),
                        extension_metadata: BTreeMap::new(),
                        hooks: Vec::new(),
                    },
                )
                .await?;
                let refreshed = self.refresh_catalog().await?;
                let skill = refreshed.resolve(&canonical).ok_or_else(|| {
                    ToolError::invalid_state("created skill missing after refresh")
                })?;
                SkillManageOutput {
                    action: "create".to_string(),
                    skill: Some(skill_summary(&skill)),
                    file_path: None,
                    note: SKILL_MANAGE_REFRESH_NOTE.to_string(),
                }
            }
            SkillManageToolInput::Edit {
                name,
                description,
                aliases,
                tags,
                instruction,
                platforms,
                requires_tools,
                fallback_for_tools,
            } => {
                let existing = managed_skill(&self.catalog, &name)?;
                write_skill_manifest(
                    &existing.root_dir,
                    &ManagedSkillSpec {
                        name: existing.name.clone(),
                        description,
                        aliases,
                        tags,
                        instruction,
                        platforms,
                        requires_tools,
                        fallback_for_tools,
                        metadata: existing.metadata.clone(),
                        extension_metadata: existing.extension_metadata.clone(),
                        hooks: existing.hooks.clone(),
                    },
                )
                .await?;
                let refreshed = self.refresh_catalog().await?;
                let skill = refreshed.resolve(&existing.name).ok_or_else(|| {
                    ToolError::invalid_state("edited skill missing after refresh")
                })?;
                SkillManageOutput {
                    action: "edit".to_string(),
                    skill: Some(skill_summary(&skill)),
                    file_path: None,
                    note: SKILL_MANAGE_REFRESH_NOTE.to_string(),
                }
            }
            SkillManageToolInput::Patch {
                name,
                description,
                aliases,
                tags,
                instruction,
                platforms,
                requires_tools,
                fallback_for_tools,
            } => {
                let existing = managed_skill(&self.catalog, &name)?;
                write_skill_manifest(
                    &existing.root_dir,
                    &ManagedSkillSpec {
                        name: existing.name.clone(),
                        description: description.unwrap_or(existing.description.clone()),
                        aliases: aliases.unwrap_or(existing.aliases.clone()),
                        tags: tags.unwrap_or(existing.tags.clone()),
                        instruction: instruction.unwrap_or(existing.body.clone()),
                        platforms: platforms.unwrap_or(existing.activation.platforms.clone()),
                        requires_tools: requires_tools.unwrap_or_else(|| {
                            existing
                                .activation
                                .requires_tools
                                .iter()
                                .map(ToString::to_string)
                                .collect()
                        }),
                        fallback_for_tools: fallback_for_tools.unwrap_or_else(|| {
                            existing
                                .activation
                                .fallback_for_tools
                                .iter()
                                .map(ToString::to_string)
                                .collect()
                        }),
                        metadata: existing.metadata.clone(),
                        extension_metadata: existing.extension_metadata.clone(),
                        hooks: existing.hooks.clone(),
                    },
                )
                .await?;
                let refreshed = self.refresh_catalog().await?;
                let skill = refreshed.resolve(&existing.name).ok_or_else(|| {
                    ToolError::invalid_state("patched skill missing after refresh")
                })?;
                SkillManageOutput {
                    action: "patch".to_string(),
                    skill: Some(skill_summary(&skill)),
                    file_path: None,
                    note: SKILL_MANAGE_REFRESH_NOTE.to_string(),
                }
            }
            SkillManageToolInput::Delete { name } => {
                let existing = managed_skill(&self.catalog, &name)?;
                fs::remove_dir_all(&existing.root_dir).await?;
                self.refresh_catalog().await?;
                SkillManageOutput {
                    action: "delete".to_string(),
                    skill: None,
                    file_path: None,
                    note: format!(
                        "Deleted skill `{}`. {}",
                        existing.name, SKILL_MANAGE_REFRESH_NOTE
                    ),
                }
            }
            SkillManageToolInput::WriteFile {
                name,
                file_path,
                content,
            } => {
                let existing = managed_skill(&self.catalog, &name)?;
                let relative = normalize_relative_skill_path(&file_path)?;
                reject_manifest_write(&relative)?;
                let absolute_path = existing.root_dir.join(&relative);
                if let Some(parent) = absolute_path.parent() {
                    fs::create_dir_all(parent).await?;
                }
                ctx.assert_path_write_allowed(&absolute_path)?;
                fs::write(&absolute_path, content).await?;
                let refreshed = self.refresh_catalog().await?;
                let skill = refreshed.resolve(&existing.name).ok_or_else(|| {
                    ToolError::invalid_state("skill missing after file write refresh")
                })?;
                SkillManageOutput {
                    action: "write_file".to_string(),
                    skill: Some(skill_summary(&skill)),
                    file_path: Some(relative),
                    note: SKILL_MANAGE_REFRESH_NOTE.to_string(),
                }
            }
            SkillManageToolInput::RemoveFile { name, file_path } => {
                let existing = managed_skill(&self.catalog, &name)?;
                let relative = normalize_relative_skill_path(&file_path)?;
                reject_manifest_write(&relative)?;
                let absolute_path = existing.root_dir.join(&relative);
                ctx.assert_path_write_allowed(&absolute_path)?;
                fs::remove_file(&absolute_path).await?;
                let refreshed = self.refresh_catalog().await?;
                let skill = refreshed.resolve(&existing.name).ok_or_else(|| {
                    ToolError::invalid_state("skill missing after file removal refresh")
                })?;
                SkillManageOutput {
                    action: "remove_file".to_string(),
                    skill: Some(skill_summary(&skill)),
                    file_path: Some(relative),
                    note: SKILL_MANAGE_REFRESH_NOTE.to_string(),
                }
            }
        };

        let mut parts = vec![MessagePart::text(render_skill_manage_result(&output))];
        if let Some(skill) = output.skill.as_ref() {
            parts.push(MessagePart::reference(
                "skill",
                Some(skill.name.clone()),
                Some(skill.skill_path.clone()),
                Some(skill.description.clone()),
            ));
        }
        parts.push(MessagePart::text(output.note.clone()));

        Ok(ToolResult {
            id: call_id.clone(),
            call_id: types::CallId::from(&call_id),
            tool_name: "skill_manage".into(),
            parts,
            attachments: Vec::new(),
            structured_content: Some(
                serde_json::to_value(output).expect("skill_manage structured output"),
            ),
            continuation: None,
            metadata: None,
            is_error: false,
        })
    }
}

impl SkillManageTool {
    async fn refresh_catalog(&self) -> Result<SkillCatalog> {
        let roots = self.catalog.roots();
        let refreshed = load_skill_roots(&roots).await.map_err(|error| {
            ToolError::invalid_state(format!("failed to refresh skill catalog: {error}"))
        })?;
        self.catalog.replace(roots, refreshed.all());
        Ok(self.catalog.clone())
    }
}

#[derive(Clone)]
struct ManagedSkillSpec {
    name: String,
    description: String,
    aliases: Vec<String>,
    tags: Vec<String>,
    instruction: String,
    platforms: Vec<String>,
    requires_tools: Vec<String>,
    fallback_for_tools: Vec<String>,
    metadata: BTreeMap<String, serde_yaml::Value>,
    extension_metadata: BTreeMap<String, serde_yaml::Value>,
    hooks: Vec<types::HookRegistration>,
}

#[derive(Serialize)]
struct ManagedSkillFrontmatter<'a> {
    name: &'a str,
    description: &'a str,
    #[serde(skip_serializing_if = "slice_is_empty")]
    aliases: &'a [String],
    #[serde(skip_serializing_if = "slice_is_empty")]
    tags: &'a [String],
    #[serde(skip_serializing_if = "slice_is_empty")]
    platforms: &'a [String],
    #[serde(skip_serializing_if = "slice_is_empty")]
    requires_tools: &'a [String],
    #[serde(skip_serializing_if = "slice_is_empty")]
    fallback_for_tools: &'a [String],
    #[serde(rename = "x-agent-core")]
    agent_core: ManagedSkillFrontmatterCore<'a>,
    #[serde(flatten)]
    metadata: &'a BTreeMap<String, serde_yaml::Value>,
}

#[derive(Serialize)]
struct ManagedSkillFrontmatterCore<'a> {
    #[serde(skip_serializing_if = "slice_is_empty")]
    hooks: &'a [types::HookRegistration],
    #[serde(flatten)]
    extension_metadata: &'a BTreeMap<String, serde_yaml::Value>,
}

fn slice_is_empty<T>(value: &[T]) -> bool {
    value.is_empty()
}

async fn write_skill_manifest(skill_dir: &Path, spec: &ManagedSkillSpec) -> Result<()> {
    let frontmatter = ManagedSkillFrontmatter {
        name: &spec.name,
        description: &spec.description,
        aliases: &spec.aliases,
        tags: &spec.tags,
        platforms: &spec.platforms,
        requires_tools: &spec.requires_tools,
        fallback_for_tools: &spec.fallback_for_tools,
        agent_core: ManagedSkillFrontmatterCore {
            hooks: &spec.hooks,
            extension_metadata: &spec.extension_metadata,
        },
        metadata: &spec.metadata,
    };
    let yaml = serde_yaml::to_string(&frontmatter).map_err(|error| {
        ToolError::invalid_state(format!("failed to serialize skill frontmatter: {error}"))
    })?;
    let content = format!("---\n{}---\n\n{}\n", yaml, spec.instruction.trim());
    fs::write(skill_dir.join("SKILL.md"), content).await?;
    Ok(())
}

fn fuzzy_skill_matches(catalog: &SkillCatalog, query: &str, limit: usize) -> Vec<SkillSummary> {
    let query = query.to_ascii_lowercase();
    let mut matches = catalog
        .all()
        .into_iter()
        .filter(|skill| {
            skill.name.to_ascii_lowercase().contains(&query)
                || skill
                    .aliases
                    .iter()
                    .any(|alias| alias.to_ascii_lowercase().contains(&query))
                || skill
                    .tags
                    .iter()
                    .any(|tag| tag.to_ascii_lowercase().contains(&query))
                || skill.description.to_ascii_lowercase().contains(&query)
        })
        .map(|skill| skill_summary(&skill))
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| left.name.cmp(&right.name));
    matches.truncate(limit);
    matches
}

fn build_detail_parts(detail: &SkillDetail) -> Vec<MessagePart> {
    let mut parts = vec![
        MessagePart::text(render_skill_detail(detail)),
        MessagePart::reference(
            "skill",
            Some(detail.summary.name.clone()),
            Some(detail.summary.skill_path.clone()),
            Some(detail.summary.description.clone()),
        ),
    ];
    if let Some(instruction) = detail.instruction.as_deref() {
        parts.push(MessagePart::text(format!("Instruction\n{instruction}")));
    }
    parts
}

fn render_skill_list(query: Option<&str>, skills: &[SkillSummary]) -> String {
    let mut lines = vec![match query {
        Some(query) => format!("Skills matching `{query}` ({})", skills.len()),
        None => format!("Available skills ({})", skills.len()),
    }];
    if skills.is_empty() {
        lines.push("No matching skills are currently loaded.".to_string());
        return lines.join("\n");
    }
    for skill in skills {
        let aliases = if skill.aliases.is_empty() {
            String::new()
        } else {
            format!(" · aliases: {}", skill.aliases.join(", "))
        };
        let tags = if skill.tags.is_empty() {
            String::new()
        } else {
            format!(" · tags: {}", skill.tags.join(", "))
        };
        lines.push(format!(
            "- /{} · {}{}{}",
            skill.name, skill.description, aliases, tags
        ));
    }
    lines.join("\n")
}

fn render_skill_detail(detail: &SkillDetail) -> String {
    let mut lines = vec![
        format!("Skill {}", detail.summary.name),
        detail.summary.description.clone(),
        format!("Slash Command /{}", detail.summary.name),
        format!(
            "Source {} · writable={}",
            detail.summary.root_kind, detail.summary.writable
        ),
    ];
    if !detail.references.is_empty() {
        lines.push(format!("References {}", detail.references.join(", ")));
    }
    if !detail.scripts.is_empty() {
        lines.push(format!("Scripts {}", detail.scripts.join(", ")));
    }
    if !detail.assets.is_empty() {
        lines.push(format!("Assets {}", detail.assets.join(", ")));
    }
    lines.join("\n")
}

fn render_skill_manage_result(output: &SkillManageOutput) -> String {
    let mut lines = vec![format!("Skill Manage {}", output.action)];
    if let Some(skill) = output.skill.as_ref() {
        lines.push(format!("Skill {}", skill.name));
    }
    if let Some(file_path) = output.file_path.as_deref() {
        lines.push(format!("File {}", file_path));
    }
    lines.push(output.note.clone());
    lines.join("\n")
}

fn skill_summary(skill: &Skill) -> SkillSummary {
    SkillSummary {
        name: skill.name.clone(),
        description: skill.description.clone(),
        aliases: skill.aliases.clone(),
        tags: skill.tags.clone(),
        skill_path: skill.skill_path().display().to_string(),
        slash_command: format!("/{}", skill.name),
        root_kind: match skill.provenance.root.kind {
            SkillRootKind::Managed => "managed".to_string(),
            SkillRootKind::External => "external".to_string(),
        },
        writable: skill.provenance.root.writable(),
        platforms: skill.activation.platforms.clone(),
        requires_tools: skill
            .activation
            .requires_tools
            .iter()
            .map(ToString::to_string)
            .collect(),
        fallback_for_tools: skill
            .activation
            .fallback_for_tools
            .iter()
            .map(ToString::to_string)
            .collect(),
    }
}

fn skill_detail(skill: &Skill, include_instruction: bool) -> SkillDetail {
    SkillDetail {
        summary: skill_summary(skill),
        references: relative_paths(&skill.root_dir, &skill.references),
        scripts: relative_paths(&skill.root_dir, &skill.scripts),
        assets: relative_paths(&skill.root_dir, &skill.assets),
        instruction: include_instruction.then(|| skill.system_instruction()),
    }
}

fn managed_skill(catalog: &SkillCatalog, query: &str) -> Result<Skill> {
    let skill = catalog
        .resolve(query)
        .ok_or_else(|| ToolError::invalid(format!("unknown skill `{query}`")))?;
    if !skill.provenance.root.writable() {
        return Err(ToolError::invalid(format!(
            "skill `{}` is loaded from a read-only root",
            skill.name
        )));
    }
    Ok(skill)
}

fn relative_paths(root: &Path, paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| {
            path.strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

fn validate_skill_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ToolError::invalid("skill name cannot be empty"));
    }
    if trimmed
        .chars()
        .any(|ch| ch == '/' || ch == '\\' || ch.is_whitespace())
    {
        return Err(ToolError::invalid(
            "skill name must not contain path separators or whitespace",
        ));
    }
    Ok(trimmed.to_string())
}

fn normalize_relative_skill_path(path: &str) -> Result<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(ToolError::invalid("skill file path cannot be empty"));
    }
    let candidate = Path::new(trimmed);
    let mut normalized = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ToolError::invalid(format!(
                    "skill file path must stay inside the skill root: {trimmed}"
                )));
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(ToolError::invalid("skill file path cannot be empty"));
    }
    Ok(normalized.to_string_lossy().replace('\\', "/"))
}

fn reject_manifest_write(relative_path: &str) -> Result<()> {
    if relative_path.eq_ignore_ascii_case("SKILL.md") {
        return Err(ToolError::invalid(
            "edit, patch, or create must be used for SKILL.md; write_file/remove_file are for companion files only",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{SkillManageTool, SkillViewTool, SkillsListTool, Tool};
    use serde_json::json;
    use skills::{SkillRoot, load_skill_roots};
    use tempfile::tempdir;
    use tokio::fs;
    use types::ToolCallId;

    async fn catalog() -> skills::SkillCatalog {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let managed_root = root.join("managed");
        let external_root = root.join("external");
        let managed_skill = managed_root.join("pdf");
        let external_skill = external_root.join("docs");
        fs::create_dir_all(managed_skill.join("references"))
            .await
            .unwrap();
        fs::create_dir_all(&external_skill).await.unwrap();
        fs::write(
            managed_skill.join("SKILL.md"),
            r#"---
name: pdf
description: Use for PDF tasks
aliases: [acrobat]
tags: [document]
---

Inspect PDFs carefully.
"#,
        )
        .await
        .unwrap();
        fs::write(managed_skill.join("references").join("guide.md"), "guide")
            .await
            .unwrap();
        fs::write(
            external_skill.join("SKILL.md"),
            r#"---
name: docs
description: Use for docs
---

Read docs carefully.
"#,
        )
        .await
        .unwrap();
        std::mem::forget(dir);
        load_skill_roots(&[
            SkillRoot::managed(managed_root),
            SkillRoot::external(external_root),
        ])
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn skills_list_surfaces_slash_commands_and_root_kind() {
        let tool = SkillsListTool::new(catalog().await);
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({}),
                &crate::ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["result_count"], 2);
        assert_eq!(structured["skills"][0]["slash_command"], "/docs");
    }

    #[tokio::test]
    async fn skill_view_can_load_companion_file() {
        let tool = SkillViewTool::new(catalog().await);
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "name": "pdf",
                    "file_path": "references/guide.md"
                }),
                &crate::ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["kind"], "file");
        assert_eq!(structured["file"]["content"], "guide");
    }

    #[tokio::test]
    async fn skill_manage_create_refreshes_catalog_and_emits_note() {
        let catalog = catalog().await;
        let tool = SkillManageTool::new(catalog.clone());
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "action": "create",
                    "name": "review",
                    "description": "Use for reviews",
                    "instruction": "Review carefully."
                }),
                &crate::ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert!(result.text_content().contains("Skill catalog changed"));
        assert!(catalog.resolve("review").is_some());
    }

    #[tokio::test]
    async fn skill_manage_rejects_mutating_external_skill() {
        let tool = SkillManageTool::new(catalog().await);
        let error = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "action": "delete",
                    "name": "docs"
                }),
                &crate::ToolExecutionContext::default(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("read-only root"));
    }
}
