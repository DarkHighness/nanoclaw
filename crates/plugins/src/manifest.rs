use serde::de::{self, Deserializer};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use toml::map::Map;
use types::{HookEvent, HookHandlerKind, HookHostApiGrant, HookMutationPermission};

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
    pub tool_roots: Vec<String>,
    #[serde(default)]
    pub hook_files: Vec<String>,
    #[serde(default)]
    pub mcp_files: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRuntimeSpec {
    pub driver: String,
    #[serde(default)]
    pub module: Option<String>,
    #[serde(default)]
    pub abi: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginMessageMutationCapability {
    Append,
    Replace,
    Patch,
    Remove,
    InsertBefore,
    InsertAfter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginToolPolicyCapability {
    Deny,
    RewriteArgs,
    PermissionDecision,
    Gate,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PluginCapabilitySet {
    #[serde(default)]
    pub hook_handlers: Vec<HookHandlerKind>,
    #[serde(default)]
    pub message_mutations: Vec<PluginMessageMutationCapability>,
    #[serde(default)]
    pub tool_policies: Vec<PluginToolPolicyCapability>,
    #[serde(default)]
    pub host_api: Vec<HookHostApiGrant>,
    #[serde(default)]
    pub mcp_exports: bool,
    #[serde(default)]
    pub skill_exports: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginNetworkAccess {
    Deny,
    Allow,
    AllowDomains(Vec<String>),
}

impl Default for PluginNetworkAccess {
    fn default() -> Self {
        Self::Deny
    }
}

impl Serialize for PluginNetworkAccess {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Deny => serializer.serialize_str("deny"),
            Self::Allow => serializer.serialize_str("allow"),
            Self::AllowDomains(domains) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("allow_domains", domains)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for PluginNetworkAccess {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum RawPluginNetworkAccess {
            Mode(String),
            AllowDomains { allow_domains: Vec<String> },
        }

        match RawPluginNetworkAccess::deserialize(deserializer)? {
            RawPluginNetworkAccess::Mode(mode) => match mode.as_str() {
                "deny" => Ok(PluginNetworkAccess::Deny),
                "allow" => Ok(PluginNetworkAccess::Allow),
                other => Err(de::Error::custom(format!(
                    "unsupported plugin network access mode `{other}`"
                ))),
            },
            RawPluginNetworkAccess::AllowDomains { allow_domains } => {
                Ok(PluginNetworkAccess::AllowDomains(allow_domains))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PluginPermissionRequest {
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
    #[serde(default)]
    pub hook_events: Vec<HookEvent>,
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
    pub components: PluginComponents,
    #[serde(default)]
    pub runtime: Option<PluginRuntimeSpec>,
    #[serde(default)]
    pub capabilities: PluginCapabilitySet,
    #[serde(default)]
    pub permissions: PluginPermissionRequest,
    #[serde(default)]
    pub instructions: Vec<PluginInstruction>,
    #[serde(default)]
    pub defaults: Map<String, toml::Value>,
}
