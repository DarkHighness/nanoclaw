use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default = "default_embedding_batch_size")]
    pub batch_size: usize,
    #[serde(default = "default_embedding_timeout_ms")]
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmServiceConfig {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default = "default_generation_timeout_ms")]
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryExpansionConfig {
    #[serde(flatten)]
    pub service: LlmServiceConfig,
    #[serde(default = "default_query_expansion_variants")]
    pub variants: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RerankConfig {
    #[serde(flatten)]
    pub service: LlmServiceConfig,
}

fn default_embedding_batch_size() -> usize {
    16
}

fn default_embedding_timeout_ms() -> u64 {
    30_000
}

fn default_generation_timeout_ms() -> u64 {
    30_000
}

fn default_query_expansion_variants() -> usize {
    1
}
