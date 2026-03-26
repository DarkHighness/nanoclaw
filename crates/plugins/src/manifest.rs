use serde::{Deserialize, Serialize};
use toml::map::Map;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PluginKind {
    #[default]
    Bundle,
    Memory,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PluginInstruction {
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PluginComponents {
    #[serde(default)]
    pub skill_roots: Vec<String>,
    #[serde(default)]
    pub hook_files: Vec<String>,
    #[serde(default)]
    pub mcp_files: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub kind: PluginKind,
    #[serde(default)]
    pub enabled_by_default: bool,
    #[serde(default)]
    pub driver: Option<String>,
    #[serde(default)]
    pub components: PluginComponents,
    #[serde(default)]
    pub instructions: Vec<PluginInstruction>,
    #[serde(default)]
    pub defaults: Map<String, toml::Value>,
}
