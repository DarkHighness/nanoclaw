use crate::manifest::PluginNetworkAccess;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use toml::map::Map;
use types::{HookHostApiGrant, HookMutationPermission, PluginId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PluginSlotsConfig {
    #[serde(default)]
    pub memory: Option<PluginId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PluginPermissionGrant {
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub write: Vec<String>,
    #[serde(default)]
    pub exec: Vec<String>,
    #[serde(default)]
    pub network: PluginNetworkAccess,
    #[serde(default)]
    pub message_mutation: HookMutationPermission,
    #[serde(default)]
    pub host_api: Vec<HookHostApiGrant>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct PluginEntryConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub permissions: PluginPermissionGrant,
    #[serde(default)]
    pub config: Map<String, toml::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginResolverConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub allow: Vec<PluginId>,
    #[serde(default)]
    pub deny: Vec<PluginId>,
    #[serde(default)]
    pub entries: BTreeMap<PluginId, PluginEntryConfig>,
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
