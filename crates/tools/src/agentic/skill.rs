use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::registry::Tool;
use crate::{Result, ToolExecutionContext};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use skills::{Skill, SkillCatalog};
use std::path::Path;
use types::{MessagePart, ToolCallId, ToolOutputMode, ToolResult, ToolSpec};

const DEFAULT_SKILL_LIMIT: usize = 20;
const MAX_SKILL_LIMIT: usize = 100;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct SkillToolInput {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub include_instruction: bool,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub skill_path: String,
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
#[serde(tag = "kind", rename_all = "snake_case")]
enum SkillToolOutput {
    List {
        query: Option<String>,
        result_count: usize,
        skills: Vec<SkillSummary>,
    },
    Detail {
        query: String,
        skill: SkillDetail,
    },
}

#[derive(Clone)]
pub struct SkillTool {
    catalog: SkillCatalog,
}

impl SkillTool {
    #[must_use]
    pub fn new(catalog: SkillCatalog) -> Self {
        Self { catalog }
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "skill",
            "Discover workspace skills or inspect one skill's instructions, aliases, and companion files. Provide `name` to resolve a specific skill or alias; omit it to list available skills.",
            serde_json::to_value(schema_for!(SkillToolInput)).expect("skill schema"),
            ToolOutputMode::ContentParts,
            tool_approval_profile(true, false, true, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(SkillToolOutput)).expect("skill output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        _ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: SkillToolInput = serde_json::from_value(arguments)?;
        let limit = input
            .limit
            .unwrap_or(DEFAULT_SKILL_LIMIT)
            .clamp(1, MAX_SKILL_LIMIT);

        let (parts, structured, metadata) = if let Some(query) = input
            .name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if let Some(skill) = self.catalog.resolve(query) {
                let detail = skill_detail(skill, input.include_instruction);
                let skill_path = detail.summary.skill_path.clone();
                let instruction_preview = detail.instruction.clone();
                let parts = build_detail_parts(&detail);
                let structured = SkillToolOutput::Detail {
                    query: query.to_string(),
                    skill: detail.clone(),
                };
                let metadata = serde_json::json!({
                    "query": query,
                    "skill_path": skill_path,
                    "references": detail.references,
                    "scripts": detail.scripts,
                    "assets": detail.assets,
                    "instruction_chars": instruction_preview.as_ref().map_or(0, String::len),
                });
                (parts, structured, metadata)
            } else {
                let matches = fuzzy_skill_matches(&self.catalog, query, limit);
                let parts = vec![MessagePart::text(render_skill_list(Some(query), &matches))];
                let structured = SkillToolOutput::List {
                    query: Some(query.to_string()),
                    result_count: matches.len(),
                    skills: matches.clone(),
                };
                let metadata = serde_json::json!({
                    "query": query,
                    "result_count": matches.len(),
                    "skills": matches,
                });
                (parts, structured, metadata)
            }
        } else {
            let skills = self
                .catalog
                .all()
                .iter()
                .take(limit)
                .map(skill_summary)
                .collect::<Vec<_>>();
            let parts = vec![MessagePart::text(render_skill_list(None, &skills))];
            let structured = SkillToolOutput::List {
                query: None,
                result_count: skills.len(),
                skills: skills.clone(),
            };
            let metadata = serde_json::json!({
                "result_count": skills.len(),
                "skills": skills,
            });
            (parts, structured, metadata)
        };

        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "skill".into(),
            parts,
            attachments: Vec::new(),
            structured_content: Some(
                serde_json::to_value(structured).expect("skill structured output"),
            ),
            continuation: None,
            metadata: Some(metadata),
            is_error: false,
        })
    }
}

fn fuzzy_skill_matches(catalog: &SkillCatalog, query: &str, limit: usize) -> Vec<SkillSummary> {
    let query = query.to_ascii_lowercase();
    let mut matches = catalog
        .all()
        .iter()
        .filter(|skill| {
            skill.name.to_ascii_lowercase().contains(&query)
                || skill
                    .aliases
                    .iter()
                    .any(|alias| alias.to_ascii_lowercase().contains(&query))
                || skill.description.to_ascii_lowercase().contains(&query)
        })
        .map(skill_summary)
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
        Some(query) => format!("[skill query={query} results={}]", skills.len()),
        None => format!("[skill results={}]", skills.len()),
    }];
    if skills.is_empty() {
        lines.push("No matching skills are currently loaded.".to_string());
        return lines.join("\n");
    }
    for (index, skill) in skills.iter().enumerate() {
        let aliases = if skill.aliases.is_empty() {
            String::new()
        } else {
            format!(" aliases={}", skill.aliases.join(","))
        };
        let tags = if skill.tags.is_empty() {
            String::new()
        } else {
            format!(" tags={}", skill.tags.join(","))
        };
        lines.push(format!(
            "{}. {}: {}{}{}",
            index + 1,
            skill.name,
            skill.description,
            aliases,
            tags
        ));
        lines.push(format!("   path> {}", skill.skill_path));
    }
    lines.join("\n")
}

fn render_skill_detail(detail: &SkillDetail) -> String {
    let mut lines = vec![
        format!(
            "[skill name={} aliases={} tags={}]",
            detail.summary.name,
            if detail.summary.aliases.is_empty() {
                "-".to_string()
            } else {
                detail.summary.aliases.join(",")
            },
            if detail.summary.tags.is_empty() {
                "-".to_string()
            } else {
                detail.summary.tags.join(",")
            }
        ),
        format!("description> {}", detail.summary.description),
        format!("path> {}", detail.summary.skill_path),
    ];
    if !detail.references.is_empty() {
        lines.push(format!("references> {}", detail.references.join(", ")));
    }
    if !detail.scripts.is_empty() {
        lines.push(format!("scripts> {}", detail.scripts.join(", ")));
    }
    if !detail.assets.is_empty() {
        lines.push(format!("assets> {}", detail.assets.join(", ")));
    }
    if detail.instruction.is_some() {
        lines.push("instruction> included below".to_string());
    }
    lines.join("\n")
}

fn skill_summary(skill: &Skill) -> SkillSummary {
    SkillSummary {
        name: skill.name.clone(),
        description: skill.description.clone(),
        aliases: skill.aliases.clone(),
        tags: skill.tags.clone(),
        skill_path: skill.root_dir.join("SKILL.md").display().to_string(),
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

fn relative_paths(root: &Path, paths: &[std::path::PathBuf]) -> Vec<String> {
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

#[cfg(test)]
mod tests {
    use super::{SkillTool, Tool};
    use serde_json::json;
    use skills::load_skill_roots;
    use tempfile::tempdir;
    use tokio::fs;
    use types::ToolCallId;

    async fn catalog() -> skills::SkillCatalog {
        let dir = tempdir().unwrap();
        let skill_dir = dir.path().join("skills").join("pdf");
        fs::create_dir_all(skill_dir.join("references"))
            .await
            .unwrap();
        fs::create_dir_all(skill_dir.join("scripts")).await.unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
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
        fs::write(skill_dir.join("references").join("guide.md"), "guide")
            .await
            .unwrap();
        fs::write(skill_dir.join("scripts").join("inspect.sh"), "echo inspect")
            .await
            .unwrap();
        load_skill_roots(&[dir.path().join("skills")])
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn listing_skills_surfaces_loaded_catalog() {
        let tool = SkillTool::new(catalog().await);
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({}),
                &crate::ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        assert!(result.text_content().contains("[skill results=1]"));
        assert!(result.text_content().contains("pdf: Use for PDF tasks"));
        assert_eq!(result.structured_content.as_ref().unwrap()["kind"], "list");
    }

    #[tokio::test]
    async fn resolving_alias_returns_skill_detail_and_instruction() {
        let tool = SkillTool::new(catalog().await);
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "name": "acrobat",
                    "include_instruction": true
                }),
                &crate::ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        assert!(result.text_content().contains("[skill name=pdf"));
        assert!(result.text_content().contains("Instruction"));
        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["kind"], "detail");
        assert_eq!(structured["skill"]["summary"]["name"], "pdf");
        assert_eq!(structured["skill"]["references"][0], "references/guide.md");
    }

    #[tokio::test]
    async fn unknown_skill_falls_back_to_filtered_list() {
        let tool = SkillTool::new(catalog().await);
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "name": "doc"
                }),
                &crate::ToolExecutionContext::default(),
            )
            .await
            .unwrap();

        assert_eq!(result.structured_content.as_ref().unwrap()["kind"], "list");
        assert_eq!(
            result.structured_content.as_ref().unwrap()["result_count"],
            1
        );
    }

    #[tokio::test]
    async fn limit_is_clamped() {
        let tool = SkillTool::new(catalog().await);
        let error = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "limit": 0
                }),
                &crate::ToolExecutionContext::default(),
            )
            .await
            .unwrap();
        assert!(error.text_content().contains("[skill results=1]"));
    }
}
