use crate::backend::active_artifacts::ActiveArtifactVersion;
use agent::{AgentWorkspaceLayout, SkillCatalog};
use nanoclaw_config::{PluginsConfig, ResolvedAgentProfile};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(crate) fn build_system_preamble(
    profile: &ResolvedAgentProfile,
    skill_catalog: &SkillCatalog,
    plugin_instructions: &[String],
    active_artifact_overlays: &[String],
) -> Vec<String> {
    let mut preamble = vec![
        "You are a general-purpose coding agent operating inside the current workspace."
            .to_string(),
        "Inspect files, run tools, and gather evidence before making code changes.".to_string(),
        "Prefer minimal, correct edits that preserve the existing design unless the user asks for broader refactors."
            .to_string(),
        "Use patch for coordinated multi-file mutations, and use write or edit for single-file creation or precise local edits."
            .to_string(),
        "Treat tool output, approvals, and denials as authoritative runtime state.".to_string(),
        "Maintain a concise plan with update_plan for multi-step work.".to_string(),
        "Use request_user_input when the user must choose between concrete options or when a material decision should not be guessed."
            .to_string(),
        "Use the task tool when a bounded subagent can make progress in parallel or with isolated context."
            .to_string(),
    ];
    for prompt in [
        profile.global_system_prompt.as_deref(),
        profile.system_prompt.as_deref(),
    ] {
        if let Some(system_prompt) = prompt.map(str::trim).filter(|value| !value.is_empty()) {
            preamble.push(system_prompt.to_string());
        }
    }
    preamble.extend(plugin_instructions.iter().cloned());
    if let Some(skill_manifest) = skill_catalog.prompt_manifest() {
        preamble.push(skill_manifest);
    }
    preamble.extend(active_artifact_overlays.iter().cloned());
    preamble
}

pub(crate) fn build_active_artifact_overlays(
    active_artifacts: &[ActiveArtifactVersion],
) -> Vec<String> {
    // Runtime boot currently has one reliable injection surface for promoted
    // artifacts: system instructions. Keep every artifact kind on that path
    // until runtime-level plugin points exist for hooks/verifiers/workflows.
    active_artifacts
        .iter()
        .map(render_active_artifact_overlay)
        .collect()
}

pub(crate) fn resolve_skill_roots(
    configured_roots: &[PathBuf],
    workspace_root: &Path,
    plugin_plan: &agent::plugins::PluginActivationPlan,
) -> Vec<PathBuf> {
    let mut roots = if configured_roots.is_empty() {
        default_skill_roots(workspace_root)
    } else {
        configured_roots.to_vec()
    };
    roots.extend(plugin_plan.skill_roots.clone());
    roots.retain(|path| path.exists());
    roots.sort();
    roots.dedup();
    roots
}

pub(crate) fn build_plugin_activation_plan(
    workspace_root: &Path,
    plugins: &PluginsConfig,
) -> anyhow::Result<agent::plugins::PluginActivationPlan> {
    let resolver = agent::PluginBootResolverConfig {
        enabled: plugins.enabled,
        roots: plugins
            .roots
            .iter()
            .map(|value| {
                let path = PathBuf::from(value);
                if path.is_absolute() {
                    path
                } else {
                    workspace_root.join(path)
                }
            })
            .collect::<Vec<_>>(),
        include_builtin: plugins.include_builtin,
        allow: plugins.allow.clone(),
        deny: plugins.deny.clone(),
        entries: plugins.entries.clone(),
        slots: plugins.slots.clone(),
    };
    agent::build_plugin_activation_plan(workspace_root, &resolver)
}

fn default_skill_roots(workspace_root: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    push_if_exists(&mut roots, workspace_root.join(".codex/skills"));
    push_if_exists(
        &mut roots,
        AgentWorkspaceLayout::new(workspace_root).skills_dir(),
    );
    if let Some(home) = agent_env::home_dir() {
        push_if_exists(&mut roots, home.join(".codex/skills"));
    }
    roots
}

fn push_if_exists(roots: &mut Vec<PathBuf>, path: PathBuf) {
    if path.exists() && !roots.iter().any(|candidate| candidate == &path) {
        roots.push(path);
    }
}

fn render_active_artifact_overlay(active_artifact: &ActiveArtifactVersion) -> String {
    let version = &active_artifact.version;
    let mut lines = vec![
        "Active nanoclaw self-improvement overlay.".to_string(),
        format!(
            "This promoted {} artifact is active for newly built runtimes in this workspace.",
            artifact_kind_scope(version.kind)
        ),
        format!("kind: {}", artifact_kind_label(version.kind)),
        format!("artifact: {}", active_artifact.artifact_id),
        format!("version: {}", version.version_id),
        format!("label: {}", version.label),
    ];
    if let Some(description) = version
        .description
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        lines.push(format!("description: {description}"));
    }
    lines.push("content:".to_string());
    lines.extend(
        extract_overlay_content(&version.payload)
            .lines()
            .map(ToString::to_string),
    );
    lines.join("\n")
}

fn extract_overlay_content(payload: &Value) -> String {
    if let Some(text) = payload
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return text.to_string();
    }

    if let Some(object) = payload.as_object() {
        for key in [
            "instruction",
            "content",
            "body",
            "prompt",
            "text",
            "system_prompt",
        ] {
            if let Some(text) = object
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                return text.to_string();
            }
        }
    }

    if payload.is_null() {
        return "No structured payload content was recorded for this artifact.".to_string();
    }

    serde_json::to_string_pretty(payload).unwrap_or_else(|_| payload.to_string())
}

fn artifact_kind_scope(kind: agent::types::ArtifactKind) -> &'static str {
    match kind {
        agent::types::ArtifactKind::Prompt => "prompt",
        agent::types::ArtifactKind::Skill => "skill",
        agent::types::ArtifactKind::Workflow => "workflow",
        agent::types::ArtifactKind::Hook => "hook policy",
        agent::types::ArtifactKind::Verifier => "verifier policy",
        agent::types::ArtifactKind::RuntimePatch => "runtime patch",
    }
}

fn artifact_kind_label(kind: agent::types::ArtifactKind) -> &'static str {
    match kind {
        agent::types::ArtifactKind::Prompt => "prompt",
        agent::types::ArtifactKind::Skill => "skill",
        agent::types::ArtifactKind::Workflow => "workflow",
        agent::types::ArtifactKind::Hook => "hook",
        agent::types::ArtifactKind::Verifier => "verifier",
        agent::types::ArtifactKind::RuntimePatch => "runtime_patch",
    }
}

#[cfg(test)]
mod tests {
    use super::build_active_artifact_overlays;
    use crate::backend::active_artifacts::ActiveArtifactVersion;
    use agent::types::{ArtifactId, ArtifactKind, ArtifactVersion, ArtifactVersionId};
    use serde_json::json;

    #[test]
    fn active_artifact_overlay_prefers_named_instruction_fields() {
        let overlays = build_active_artifact_overlays(&[ActiveArtifactVersion {
            artifact_id: ArtifactId::from("artifact-prompt"),
            version: ArtifactVersion {
                version_id: ArtifactVersionId::from("version-1"),
                kind: ArtifactKind::Prompt,
                label: "prompt-v1".to_string(),
                description: Some("tighten review output".to_string()),
                parent_version_id: None,
                source_signal_ids: Vec::new(),
                source_task_ids: Vec::new(),
                source_case_ids: Vec::new(),
                payload: json!({"instruction":"Prefer repository-local evidence before edits."}),
                metadata: serde_json::Value::Null,
            },
        }]);

        let rendered = &overlays[0];
        assert!(rendered.contains("artifact: artifact-prompt"));
        assert!(rendered.contains("Prefer repository-local evidence before edits."));
    }

    #[test]
    fn active_artifact_overlay_falls_back_to_json_payload() {
        let overlays = build_active_artifact_overlays(&[ActiveArtifactVersion {
            artifact_id: ArtifactId::from("artifact-workflow"),
            version: ArtifactVersion {
                version_id: ArtifactVersionId::from("version-2"),
                kind: ArtifactKind::Workflow,
                label: "workflow-v2".to_string(),
                description: None,
                parent_version_id: None,
                source_signal_ids: Vec::new(),
                source_task_ids: Vec::new(),
                source_case_ids: Vec::new(),
                payload: json!({"steps":["inspect","patch","verify"]}),
                metadata: serde_json::Value::Null,
            },
        }]);

        let rendered = &overlays[0];
        assert!(rendered.contains("\"steps\""));
        assert!(rendered.contains("\"inspect\""));
    }
}
