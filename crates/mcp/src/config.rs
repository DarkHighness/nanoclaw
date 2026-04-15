use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use types::McpServerName;

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum McpTransportConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        cwd: Option<String>,
    },
    StreamableHttp {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpNetworkPolicyConfig {
    Off,
    Full,
    Allowlist {
        #[serde(default)]
        domains: Vec<String>,
        #[serde(default)]
        cidrs: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: McpServerName,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap_network: Option<McpNetworkPolicyConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_network: Option<McpNetworkPolicyConfig>,
    pub transport: McpTransportConfig,
}
