use inference::{EmbeddingConfig, QueryExpansionConfig, RerankConfig};
use serde::{Deserialize, Serialize};
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
pub struct MemoryRuntimeExportConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_runtime_export_output_dir")]
    pub output_dir: PathBuf,
    #[serde(default = "default_runtime_export_max_sessions")]
    pub max_sessions: usize,
    #[serde(default = "default_runtime_export_include_search_corpus")]
    pub include_search_corpus: bool,
    #[serde(default = "default_runtime_export_max_search_corpus_chars")]
    pub max_search_corpus_chars: usize,
}

impl Default for MemoryRuntimeExportConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            output_dir: default_runtime_export_output_dir(),
            max_sessions: default_runtime_export_max_sessions(),
            include_search_corpus: default_runtime_export_include_search_corpus(),
            max_search_corpus_chars: default_runtime_export_max_search_corpus_chars(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryCorpusConfig {
    #[serde(default = "default_include_globs")]
    pub include_globs: Vec<String>,
    #[serde(default)]
    pub extra_paths: Vec<PathBuf>,
    #[serde(default)]
    pub runtime_export: MemoryRuntimeExportConfig,
}

impl Default for MemoryCorpusConfig {
    fn default() -> Self {
        Self {
            include_globs: default_include_globs(),
            extra_paths: Vec::new(),
            runtime_export: MemoryRuntimeExportConfig::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryBackgroundSyncConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_background_sync_run_on_start")]
    pub run_on_start: bool,
    #[serde(default = "default_background_sync_interval_ms")]
    pub interval_ms: u64,
}

impl Default for MemoryBackgroundSyncConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            run_on_start: default_background_sync_run_on_start(),
            interval_ms: default_background_sync_interval_ms(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryVectorStoreKind {
    #[default]
    Sqlite,
    #[serde(alias = "lancerdb")]
    Lancedb,
}

impl MemoryVectorStoreKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Lancedb => "lancedb",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryVectorStoreConfig {
    #[serde(default)]
    pub kind: MemoryVectorStoreKind,
    #[serde(default)]
    pub path: Option<PathBuf>,
}

impl Default for MemoryVectorStoreConfig {
    fn default() -> Self {
        Self {
            kind: MemoryVectorStoreKind::default(),
            path: None,
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
    pub background_sync: MemoryBackgroundSyncConfig,
}

impl Default for MemoryCoreConfig {
    fn default() -> Self {
        Self {
            corpus: MemoryCorpusConfig::default(),
            chunking: MemoryChunkingConfig::default(),
            search: MemorySearchConfig::default(),
            background_sync: MemoryBackgroundSyncConfig::default(),
        }
    }
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
    pub background_sync: MemoryBackgroundSyncConfig,
    #[serde(default)]
    pub embedding: Option<EmbeddingConfig>,
    #[serde(default)]
    pub query_expansion: Option<QueryExpansionConfig>,
    #[serde(default)]
    pub rerank: Option<RerankConfig>,
    #[serde(default)]
    pub hybrid: HybridWeights,
    #[serde(default)]
    pub vector_store: MemoryVectorStoreConfig,
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
    vec![
        "MEMORY.md".to_string(),
        "memory/**/*.md".to_string(),
        ".nanoclaw/memory/**/*.md".to_string(),
    ]
}

fn default_runtime_export_output_dir() -> PathBuf {
    PathBuf::from(".nanoclaw/memory/episodic")
}

fn default_runtime_export_max_sessions() -> usize {
    24
}

fn default_runtime_export_include_search_corpus() -> bool {
    true
}

fn default_runtime_export_max_search_corpus_chars() -> usize {
    4_096
}

fn default_background_sync_run_on_start() -> bool {
    true
}

fn default_background_sync_interval_ms() -> u64 {
    300_000
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

impl MemoryEmbedConfig {
    #[must_use]
    pub fn as_core_config(&self) -> MemoryCoreConfig {
        MemoryCoreConfig {
            corpus: self.corpus.clone(),
            chunking: self.chunking.clone(),
            search: self.search.clone(),
            background_sync: self.background_sync.clone(),
        }
    }
}
