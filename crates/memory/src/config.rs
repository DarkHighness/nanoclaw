use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryChunkingConfig {
    #[serde(default = "default_chunk_target_tokens")]
    pub target_tokens: usize,
    #[serde(default = "default_chunk_overlap_tokens")]
    pub overlap_tokens: usize,
}

impl Default for MemoryChunkingConfig {
    fn default() -> Self {
        Self {
            target_tokens: default_chunk_target_tokens(),
            overlap_tokens: default_chunk_overlap_tokens(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySearchConfig {
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    #[serde(default = "default_max_snippet_chars")]
    pub max_snippet_chars: usize,
    #[serde(default = "default_context_lines")]
    pub context_lines: usize,
}

impl Default for MemorySearchConfig {
    fn default() -> Self {
        Self {
            max_results: default_max_results(),
            max_snippet_chars: default_max_snippet_chars(),
            context_lines: default_context_lines(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryCorpusConfig {
    #[serde(default = "default_include_globs")]
    pub include_globs: Vec<String>,
    #[serde(default)]
    pub extra_paths: Vec<PathBuf>,
}

impl Default for MemoryCorpusConfig {
    fn default() -> Self {
        Self {
            include_globs: default_include_globs(),
            extra_paths: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryCoreConfig {
    #[serde(default)]
    pub corpus: MemoryCorpusConfig,
    #[serde(default)]
    pub chunking: MemoryChunkingConfig,
    #[serde(default)]
    pub search: MemorySearchConfig,
    #[serde(default)]
    pub index_path: Option<PathBuf>,
}

impl Default for MemoryCoreConfig {
    fn default() -> Self {
        Self {
            corpus: MemoryCorpusConfig::default(),
            chunking: MemoryChunkingConfig::default(),
            search: MemorySearchConfig::default(),
            index_path: None,
        }
    }
}

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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HybridWeights {
    #[serde(default = "default_vector_weight")]
    pub vector_weight: f64,
    #[serde(default = "default_text_weight")]
    pub text_weight: f64,
    #[serde(default = "default_candidate_multiplier")]
    pub candidate_multiplier: usize,
    #[serde(default = "default_rrf_k")]
    pub rrf_k: usize,
    #[serde(default = "default_top_rank_bonus_first")]
    pub top_rank_bonus_first: f64,
    #[serde(default = "default_top_rank_bonus_other")]
    pub top_rank_bonus_other: f64,
    #[serde(default = "default_rerank_top_k")]
    pub rerank_top_k: usize,
    #[serde(default)]
    pub mmr_lambda: Option<f64>,
    #[serde(default = "default_mmr_pool_k")]
    pub mmr_pool_k: usize,
}

impl Default for HybridWeights {
    fn default() -> Self {
        Self {
            vector_weight: default_vector_weight(),
            text_weight: default_text_weight(),
            candidate_multiplier: default_candidate_multiplier(),
            rrf_k: default_rrf_k(),
            top_rank_bonus_first: default_top_rank_bonus_first(),
            top_rank_bonus_other: default_top_rank_bonus_other(),
            rerank_top_k: default_rerank_top_k(),
            mmr_lambda: None,
            mmr_pool_k: default_mmr_pool_k(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct MemoryEmbedConfig {
    #[serde(default)]
    pub corpus: MemoryCorpusConfig,
    #[serde(default)]
    pub chunking: MemoryChunkingConfig,
    #[serde(default)]
    pub search: MemorySearchConfig,
    #[serde(default)]
    pub embedding: Option<EmbeddingConfig>,
    #[serde(default)]
    pub query_expansion: Option<QueryExpansionConfig>,
    #[serde(default)]
    pub rerank: Option<RerankConfig>,
    #[serde(default)]
    pub hybrid: HybridWeights,
    #[serde(default)]
    pub index_path: Option<PathBuf>,
}

fn default_chunk_target_tokens() -> usize {
    400
}

fn default_chunk_overlap_tokens() -> usize {
    80
}

fn default_max_results() -> usize {
    6
}

fn default_max_snippet_chars() -> usize {
    700
}

fn default_context_lines() -> usize {
    1
}

fn default_include_globs() -> Vec<String> {
    vec!["MEMORY.md".to_string(), "memory/**/*.md".to_string()]
}

fn default_vector_weight() -> f64 {
    0.65
}

fn default_text_weight() -> f64 {
    0.35
}

fn default_candidate_multiplier() -> usize {
    6
}

fn default_rrf_k() -> usize {
    60
}

fn default_top_rank_bonus_first() -> f64 {
    0.05
}

fn default_top_rank_bonus_other() -> f64 {
    0.02
}

fn default_rerank_top_k() -> usize {
    30
}

fn default_mmr_pool_k() -> usize {
    20
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

impl MemoryEmbedConfig {
    #[must_use]
    pub fn as_core_config(&self) -> MemoryCoreConfig {
        MemoryCoreConfig {
            corpus: self.corpus.clone(),
            chunking: self.chunking.clone(),
            search: self.search.clone(),
            index_path: self.index_path.clone(),
        }
    }
}
