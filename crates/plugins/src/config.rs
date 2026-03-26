use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use toml::map::Map;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PluginSlotsConfig {
    #[serde(default)]
    pub memory: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct PluginEntryConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub config: Map<String, toml::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginResolverConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
    #[serde(default)]
    pub entries: BTreeMap<String, PluginEntryConfig>,
    #[serde(default)]
    pub slots: PluginSlotsConfig,
}

fn default_true() -> bool {
    true
}

impl Default for PluginResolverConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow: Vec::new(),
            deny: Vec::new(),
            entries: BTreeMap::new(),
            slots: PluginSlotsConfig::default(),
        }
    }
}
