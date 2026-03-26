use crate::{
    MemoryBackend, MemoryCoreConfig, MemoryDocument, MemoryGetRequest, MemorySearchHit,
    MemorySearchRequest, MemorySearchResponse, MemorySyncStatus, Result, chunk_corpus,
    load_configured_memory_corpus,
};
use async_trait::async_trait;
use serde_json::json;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use store::RunStore;
use tools::format_numbered_lines;

pub struct MemoryCoreBackend {
    workspace_root: PathBuf,
    config: MemoryCoreConfig,
    run_store: Option<Arc<dyn RunStore>>,
}

impl MemoryCoreBackend {
    #[must_use]
    pub fn new(workspace_root: PathBuf, config: MemoryCoreConfig) -> Self {
        Self {
            workspace_root,
            config,
            run_store: None,
        }
    }

    #[must_use]
    pub fn with_run_store(mut self, run_store: Arc<dyn RunStore>) -> Self {
        self.run_store = Some(run_store);
        self
    }
}

#[async_trait]
impl MemoryBackend for MemoryCoreBackend {
    async fn sync(&self) -> Result<MemorySyncStatus> {
        let (corpus, _) = load_configured_memory_corpus(
            &self.workspace_root,
            &self.config.corpus,
            self.run_store.as_ref(),
        )
        .await?;
        Ok(MemorySyncStatus {
            backend: "memory-core".to_string(),
            indexed_documents: corpus.documents.len(),
            indexed_lines: corpus.total_lines(),
        })
    }

    async fn search(&self, req: MemorySearchRequest) -> Result<MemorySearchResponse> {
        let (corpus, runtime_exports) = load_configured_memory_corpus(
            &self.workspace_root,
            &self.config.corpus,
            self.run_store.as_ref(),
        )
        .await?;
        let chunks = chunk_corpus(&corpus, &self.config.chunking);
        let query_terms = tokenize_query(&req.query);
        let limit = req
            .limit
            .unwrap_or(self.config.search.max_results)
            .max(1)
            .min(50);
        let prefix = req.path_prefix.map(|value| value.trim().to_string());
        let mut hits = Vec::new();

        for chunk in chunks {
            if let Some(prefix) = prefix.as_deref()
                && !chunk.path.starts_with(prefix)
            {
                continue;
            }
            let lexical_score = score_text(&chunk.text, &query_terms)
                + if chunk.path == "MEMORY.md" { 0.15 } else { 0.0 };
            if lexical_score <= 0.0 {
                continue;
            }
            let mut metadata = BTreeMap::new();
            metadata.insert(
                "snapshot_id".to_string(),
                serde_json::Value::String(chunk.snapshot_id.clone()),
            );
            metadata.insert("lexical_score".to_string(), json!(lexical_score));
            hits.push(MemorySearchHit {
                hit_id: hit_id(&chunk.path, chunk.start_line),
                path: chunk.path.clone(),
                start_line: chunk.start_line,
                end_line: chunk.end_line,
                score: lexical_score,
                snippet: render_snippet(&chunk.text, self.config.search.max_snippet_chars),
                metadata,
            });
        }

        hits.sort_by(compare_hit_score);
        hits.truncate(limit);

        let mut metadata = BTreeMap::new();
        metadata.insert("query".to_string(), json!(req.query));
        metadata.insert(
            "indexed_documents".to_string(),
            json!(corpus.documents.len()),
        );
        metadata.insert(
            "runtime_exported_runs".to_string(),
            json!(runtime_exports.exported_runs),
        );
        if let Some(output_dir) = runtime_exports.output_dir {
            metadata.insert("runtime_export_dir".to_string(), json!(output_dir));
        }
        metadata.insert("fallback_used".to_string(), json!(false));
        Ok(MemorySearchResponse {
            backend: "memory-core".to_string(),
            hits,
            metadata,
        })
    }

    async fn get(&self, req: MemoryGetRequest) -> Result<MemoryDocument> {
        let (corpus, _) = load_configured_memory_corpus(
            &self.workspace_root,
            &self.config.corpus,
            self.run_store.as_ref(),
        )
        .await?;
        let requested = normalize_path(&req.path);
        let document = corpus
            .documents
            .iter()
            .find(|doc| doc.path == requested)
            .ok_or_else(|| crate::MemoryError::PathNotInCorpus(req.path.clone()))?;
        if document.lines.is_empty() {
            return Ok(MemoryDocument {
                path: document.path.clone(),
                snapshot_id: document.snapshot_id.clone(),
                requested_start_line: req.start_line.unwrap_or(1).max(1),
                resolved_start_line: 0,
                resolved_end_line: 0,
                total_lines: 0,
                text: String::new(),
            });
        }
        let start_line = req.start_line.unwrap_or(1).max(1);
        let line_count = req.line_count.unwrap_or(document.lines.len()).max(1);
        let resolved_start = start_line.min(document.lines.len().max(1));
        let resolved_end = (resolved_start + line_count - 1).min(document.lines.len());
        let text = format_numbered_lines(
            &document.lines[resolved_start - 1..resolved_end].to_vec(),
            resolved_start,
        );

        Ok(MemoryDocument {
            path: document.path.clone(),
            snapshot_id: document.snapshot_id.clone(),
            requested_start_line: start_line,
            resolved_start_line: resolved_start,
            resolved_end_line: resolved_end,
            total_lines: document.lines.len(),
            text,
        })
    }
}

fn tokenize_query(query: &str) -> Vec<String> {
    query
        .split(|ch: char| !ch.is_alphanumeric())
        .map(|value| value.to_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
}

pub(crate) fn lexical_score(query: &str, text: &str) -> f64 {
    score_text(text, &tokenize_query(query))
}

fn score_text(text: &str, terms: &[String]) -> f64 {
    if terms.is_empty() {
        return 0.0;
    }
    let text = text.to_lowercase();
    let mut score = 0.0;
    let mut seen = 0usize;
    for term in terms {
        let count = text.matches(term).count();
        if count == 0 {
            continue;
        }
        score += count as f64;
        seen += 1;
    }
    if seen == 0 {
        return 0.0;
    }
    let recall = seen as f64 / terms.len() as f64;
    // Favor chunks that cover more of the query, then add a small frequency
    // term so repeated exact tokens still separate otherwise similar chunks.
    (recall * 2.0) + (score * 0.1)
}

fn render_snippet(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut value = text
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    value.push_str("...");
    value
}

fn compare_hit_score(left: &MemorySearchHit, right: &MemorySearchHit) -> Ordering {
    right
        .score
        .partial_cmp(&left.score)
        .unwrap_or(Ordering::Equal)
        .then_with(|| left.path.cmp(&right.path))
        .then_with(|| left.start_line.cmp(&right.start_line))
}

fn hit_id(path: &str, line: usize) -> String {
    use sha2::Digest;

    let digest = sha2::Sha256::digest(format!("{path}:{line}").as_bytes());
    digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn normalize_path(value: &str) -> String {
    value.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::MemoryCoreBackend;
    use crate::{MemoryBackend, MemoryCoreConfig, MemoryGetRequest, MemorySearchRequest};
    use tempfile::tempdir;
    use tokio::fs;

    #[tokio::test]
    async fn search_returns_ranked_hits() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("MEMORY.md"),
            "primary redis sentinel\nfallback worker",
        )
        .await
        .unwrap();
        fs::create_dir_all(dir.path().join("memory")).await.unwrap();
        fs::write(
            dir.path().join("memory/today.md"),
            "redis cache decision\nanother note",
        )
        .await
        .unwrap();

        let backend = MemoryCoreBackend::new(dir.path().to_path_buf(), MemoryCoreConfig::default());
        let response = backend
            .search(MemorySearchRequest {
                query: "redis sentinel".to_string(),
                limit: Some(3),
                path_prefix: None,
            })
            .await
            .unwrap();
        assert_eq!(response.backend, "memory-core");
        assert!(!response.hits.is_empty());
        assert!(response.hits[0].score >= response.hits[response.hits.len() - 1].score);
    }

    #[tokio::test]
    async fn get_reads_line_window() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("MEMORY.md"), "line1\nline2\nline3")
            .await
            .unwrap();

        let backend = MemoryCoreBackend::new(dir.path().to_path_buf(), MemoryCoreConfig::default());
        let document = backend
            .get(MemoryGetRequest {
                path: "MEMORY.md".to_string(),
                start_line: Some(2),
                line_count: Some(1),
            })
            .await
            .unwrap();
        assert_eq!(document.resolved_start_line, 2);
        assert_eq!(document.resolved_end_line, 2);
        assert_eq!(document.text, " 2 | line2");
    }
}
