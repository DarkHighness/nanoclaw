use agent_core_types::HookRegistration;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct AgentCoreSkillExtension {
    #[serde(default)]
    pub hooks: Vec<HookRegistration>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml::Value>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(rename = "x-agent-core", default)]
    pub agent_core: AgentCoreSkillExtension,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml::Value>,
}
