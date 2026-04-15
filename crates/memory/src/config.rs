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
        "AGENTS.md".to_string(),
        "**/AGENTS.md".to_string(),
        "MEMORY.md".to_string(),
        "memory/**/*.md".to_string(),
        ".nanoclaw/memory/MEMORY.md".to_string(),
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
