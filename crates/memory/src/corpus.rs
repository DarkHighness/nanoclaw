use crate::{
    MemoryChunkingConfig, MemoryCorpusConfig, MemoryDocumentMetadata, MemoryError, MemoryScope,
    MemoryStatus, Result,
};
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::UNIX_EPOCH;
use tokio::fs;
use types::{RunId, SessionId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryCorpusDocument {
    pub path: String,
    pub absolute_path: PathBuf,
    pub snapshot_id: String,
    pub title: String,
    pub lines: Vec<String>,
    pub metadata: MemoryDocumentMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryCorpus {
    pub documents: Vec<MemoryCorpusDocument>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryCorpusChunk {
    pub path: String,
    pub snapshot_id: String,
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
    pub metadata: MemoryDocumentMetadata,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct MemoryFrontmatter {
    scope: Option<MemoryScope>,
    layer: Option<String>,
    run_id: Option<RunId>,
    session_id: Option<SessionId>,
    agent_name: Option<String>,
    task_id: Option<String>,
    updated_at_ms: Option<u64>,
    promoted_from: Option<String>,
    supersedes: Option<Vec<String>>,
    tags: Option<Vec<String>>,
    status: Option<MemoryStatus>,
}

#[derive(Clone, Debug, Default)]
struct ParsedFrontmatter {
    body_start_line: usize,
    frontmatter: Option<MemoryFrontmatter>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct MemoryCorpusCacheKey {
    workspace_root: PathBuf,
    include_globs: Vec<String>,
    extra_paths: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MemoryCorpusFileFingerprint {
    len: u64,
    modified_at_ms: Option<u64>,
    modified_at_ns: Option<u64>,
}

#[derive(Clone, Debug)]
struct CachedMemoryCorpusDocument {
    fingerprint: MemoryCorpusFileFingerprint,
    document: MemoryCorpusDocument,
}

#[derive(Clone, Debug, Default)]
struct MemoryCorpusCacheEntry {
    documents: BTreeMap<String, CachedMemoryCorpusDocument>,
}

// Memory search backends reload the same Markdown corpus repeatedly inside one
// process. Keeping an in-memory snapshot cache avoids re-reading unchanged
// files while preserving Markdown as the only source of truth.
static MEMORY_CORPUS_CACHE: OnceLock<
    Mutex<HashMap<MemoryCorpusCacheKey, Arc<MemoryCorpusCacheEntry>>>,
> = OnceLock::new();

#[cfg(test)]
static CORPUS_DISK_READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl MemoryCorpus {
    #[must_use]
    pub fn total_lines(&self) -> usize {
        self.documents.iter().map(|doc| doc.lines.len()).sum()
    }
}

pub fn chunk_corpus(
    corpus: &MemoryCorpus,
    config: &MemoryChunkingConfig,
) -> Vec<MemoryCorpusChunk> {
    let target_chars = (config.target_tokens.max(1) * 4).max(32);
    let overlap_chars = config.overlap_tokens.max(1) * 4;
    let mut chunks = Vec::new();

    for document in &corpus.documents {
        if document.lines.is_empty() {
            continue;
        }

        let mut start = 0usize;
        while start < document.lines.len() {
            let mut end = start;
            let mut char_count = 0usize;
            while end < document.lines.len() {
                let next = document.lines[end].len() + 1;
                if end > start && char_count + next > target_chars {
                    break;
                }
                char_count += next;
                end += 1;
            }

            let text = document.lines[start..end].join("\n");
            chunks.push(MemoryCorpusChunk {
                path: document.path.clone(),
                snapshot_id: document.snapshot_id.clone(),
                start_line: start + 1,
                end_line: end,
                text,
                metadata: document.metadata.clone(),
            });

            if end >= document.lines.len() {
                break;
            }

            let next_start = if end == start + 1 {
                // Tiny chunk targets can legitimately collapse to a single line.
                // In that case there is no room to preserve overlap, so we must
                // advance to `end` instead of rewinding to the same `start`.
                end
            } else {
                let mut overlap_start = end;
                let mut overlap_size = 0usize;
                while overlap_start > start + 1 {
                    let candidate = document.lines[overlap_start - 1].len() + 1;
                    if overlap_size + candidate > overlap_chars {
                        break;
                    }
                    overlap_size += candidate;
                    overlap_start -= 1;
                }
                overlap_start.min(end - 1)
            };
            start = next_start;
        }
    }

    chunks
}

pub async fn load_memory_corpus(
    workspace_root: &Path,
    config: &MemoryCorpusConfig,
) -> Result<MemoryCorpus> {
    let include_set = build_globset(&config.include_globs)?;
    let cache_key = MemoryCorpusCacheKey {
        workspace_root: workspace_root.to_path_buf(),
        include_globs: config.include_globs.clone(),
        extra_paths: config.extra_paths.clone(),
    };
    let cached = {
        memory_corpus_cache()
            .lock()
            .expect("memory corpus cache lock")
            .get(&cache_key)
            .cloned()
    };
    let mut candidates = discover_default_candidates(workspace_root, &include_set)?;
    candidates.extend(resolve_extra_paths(workspace_root, &config.extra_paths)?);
    candidates.sort();
    candidates.dedup();

    let mut documents = BTreeMap::new();
    for absolute_path in candidates {
        let Some(fingerprint) = load_file_fingerprint(&absolute_path).await? else {
            continue;
        };
        let relative = absolute_path
            .strip_prefix(workspace_root)
            .map_err(|_| MemoryError::PathOutsideWorkspace(absolute_path.display().to_string()))?;
        let relative_path = normalize_relative_path(relative);
        if !include_set.is_match(relative) && !is_extra_path(workspace_root, config, &absolute_path)
        {
            continue;
        }
        let document = if let Some(document) = reuse_cached_document(
            cached.as_deref(),
            &relative_path,
            &absolute_path,
            &fingerprint,
        ) {
            document
        } else {
            load_document_from_disk(
                absolute_path.clone(),
                &relative_path,
                fingerprint.modified_at_ms,
            )
            .await?
        };
        documents.insert(
            relative_path,
            CachedMemoryCorpusDocument {
                fingerprint,
                document,
            },
        );
    }

    let corpus = MemoryCorpus {
        documents: documents
            .values()
            .map(|cached| cached.document.clone())
            .collect(),
    };
    memory_corpus_cache()
        .lock()
        .expect("memory corpus cache lock")
        .insert(cache_key, Arc::new(MemoryCorpusCacheEntry { documents }));
    Ok(corpus)
}

fn memory_corpus_cache()
-> &'static Mutex<HashMap<MemoryCorpusCacheKey, Arc<MemoryCorpusCacheEntry>>> {
    MEMORY_CORPUS_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn reuse_cached_document(
    cached: Option<&MemoryCorpusCacheEntry>,
    relative_path: &str,
    absolute_path: &Path,
    fingerprint: &MemoryCorpusFileFingerprint,
) -> Option<MemoryCorpusDocument> {
    let cached = cached?;
    let document = cached.documents.get(relative_path)?;
    (document.document.absolute_path == absolute_path && document.fingerprint == *fingerprint)
        .then(|| document.document.clone())
}

async fn load_document_from_disk(
    absolute_path: PathBuf,
    relative_path: &str,
    modified_at_ms: Option<u64>,
) -> Result<MemoryCorpusDocument> {
    let text = read_document_text(&absolute_path).await?;
    let lines = parse_lines(&text);
    let parsed_frontmatter = parse_frontmatter(&text)?;
    let metadata = merge_metadata(
        infer_metadata_from_path(relative_path),
        parsed_frontmatter.frontmatter,
        modified_at_ms,
    );
    Ok(MemoryCorpusDocument {
        path: relative_path.to_string(),
        absolute_path,
        snapshot_id: stable_hash(&text),
        title: extract_title(&lines, parsed_frontmatter.body_start_line, relative_path),
        lines,
        metadata,
    })
}

async fn read_document_text(path: &Path) -> Result<String> {
    record_corpus_disk_read();
    Ok(fs::read_to_string(path).await?)
}

fn build_globset(globs: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in globs {
        builder.add(Glob::new(pattern)?);
    }
    Ok(builder.build()?)
}

fn discover_default_candidates(
    workspace_root: &Path,
    include_set: &GlobSet,
) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut walker = WalkBuilder::new(workspace_root);
    walker.hidden(false);
    walker.follow_links(false);
    for entry in walker.build() {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(relative) = path.strip_prefix(workspace_root) else {
            continue;
        };
        if include_set.is_match(relative) {
            paths.push(path.to_path_buf());
        }
    }
    Ok(paths)
}

fn resolve_extra_paths(workspace_root: &Path, extras: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for value in extras {
        let absolute = if value.is_absolute() {
            value.clone()
        } else {
            workspace_root.join(value)
        };
        if !absolute.starts_with(workspace_root) {
            return Err(MemoryError::PathOutsideWorkspace(
                value.display().to_string(),
            ));
        }
        if absolute.is_dir() {
            let mut walker = WalkBuilder::new(&absolute);
            walker.hidden(false);
            walker.follow_links(false);
            for entry in walker.build() {
                let Ok(entry) = entry else {
                    continue;
                };
                let path = entry.path();
                if path.is_file() && path.extension().is_some_and(|value| value == "md") {
                    out.push(path.to_path_buf());
                }
            }
        } else if absolute.is_file() {
            out.push(absolute);
        }
    }
    Ok(out)
}

fn parse_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    text.lines().map(ToOwned::to_owned).collect()
}

fn normalize_relative_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn stable_hash(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn is_extra_path(workspace_root: &Path, config: &MemoryCorpusConfig, absolute_path: &Path) -> bool {
    config
        .extra_paths
        .iter()
        .map(|path| {
            if path.is_absolute() {
                path.clone()
            } else {
                workspace_root.join(path)
            }
        })
        .any(|path| {
            if path.is_dir() {
                absolute_path.starts_with(&path)
            } else {
                absolute_path == path
            }
        })
}

fn parse_frontmatter(text: &str) -> Result<ParsedFrontmatter> {
    if !text.starts_with("---\n") && text.trim() != "---" {
        return Ok(ParsedFrontmatter {
            body_start_line: 1,
            frontmatter: None,
        });
    }

    let mut yaml_lines = Vec::new();
    let mut line_count = 1usize;
    let mut closed = false;

    for line in text.lines().skip(1) {
        line_count += 1;
        if line.trim() == "---" {
            closed = true;
            break;
        }
        yaml_lines.push(line);
    }

    if !closed {
        return Ok(ParsedFrontmatter {
            body_start_line: 1,
            frontmatter: None,
        });
    }

    let encoded = yaml_lines.join("\n");
    let frontmatter = if encoded.trim().is_empty() {
        MemoryFrontmatter::default()
    } else {
        serde_yaml::from_str::<MemoryFrontmatter>(&encoded).map_err(|error| {
            MemoryError::invalid(format!("failed to parse memory frontmatter: {error}"))
        })?
    };

    Ok(ParsedFrontmatter {
        body_start_line: line_count + 1,
        frontmatter: Some(frontmatter),
    })
}

pub(crate) fn parse_memory_text(
    path: &str,
    text: &str,
    updated_at_ms: Option<u64>,
) -> Result<(MemoryDocumentMetadata, String)> {
    let lines = parse_lines(text);
    let parsed_frontmatter = parse_frontmatter(text)?;
    let metadata = merge_metadata(
        infer_metadata_from_path(path),
        parsed_frontmatter.frontmatter,
        updated_at_ms,
    );
    let title = extract_title(&lines, parsed_frontmatter.body_start_line, path);
    Ok((metadata, title))
}

fn infer_metadata_from_path(path: &str) -> MemoryDocumentMetadata {
    if path == "MEMORY.md" {
        return MemoryDocumentMetadata {
            scope: MemoryScope::Semantic,
            layer: "curated".to_string(),
            ..MemoryDocumentMetadata::default()
        };
    }

    if let Some(stem) = file_stem(path) {
        if path.starts_with(".nanoclaw/memory/procedural/") {
            return MemoryDocumentMetadata {
                scope: MemoryScope::Procedural,
                layer: "managed-procedural".to_string(),
                ..MemoryDocumentMetadata::default()
            };
        }
        if path.starts_with(".nanoclaw/memory/semantic/") {
            return MemoryDocumentMetadata {
                scope: MemoryScope::Semantic,
                layer: "managed-semantic".to_string(),
                ..MemoryDocumentMetadata::default()
            };
        }
        if path.starts_with(".nanoclaw/memory/episodic/runs/") {
            return MemoryDocumentMetadata {
                scope: MemoryScope::Episodic,
                layer: "runtime-run".to_string(),
                run_id: Some(RunId::from(stem)),
                ..MemoryDocumentMetadata::default()
            };
        }
        if path.starts_with(".nanoclaw/memory/episodic/sessions/") {
            return MemoryDocumentMetadata {
                scope: MemoryScope::Episodic,
                layer: "runtime-session".to_string(),
                session_id: Some(SessionId::from(stem)),
                ..MemoryDocumentMetadata::default()
            };
        }
        if path.starts_with(".nanoclaw/memory/episodic/subagents/") {
            return MemoryDocumentMetadata {
                scope: MemoryScope::Episodic,
                layer: "runtime-subagent".to_string(),
                agent_name: Some(stem.to_string()),
                ..MemoryDocumentMetadata::default()
            };
        }
        if path.starts_with(".nanoclaw/memory/episodic/tasks/") {
            return MemoryDocumentMetadata {
                scope: MemoryScope::Episodic,
                layer: "runtime-task".to_string(),
                task_id: Some(stem.to_string()),
                ..MemoryDocumentMetadata::default()
            };
        }
        if path.starts_with(".nanoclaw/memory/working/sessions/") {
            return MemoryDocumentMetadata {
                scope: MemoryScope::Working,
                layer: "working-session".to_string(),
                session_id: Some(SessionId::from(stem)),
                ..MemoryDocumentMetadata::default()
            };
        }
        if path.starts_with(".nanoclaw/memory/working/tasks/") {
            return MemoryDocumentMetadata {
                scope: MemoryScope::Working,
                layer: "working-task".to_string(),
                task_id: Some(stem.to_string()),
                ..MemoryDocumentMetadata::default()
            };
        }
        if path.starts_with(".nanoclaw/memory/coordination/plans/") {
            return MemoryDocumentMetadata {
                scope: MemoryScope::Coordination,
                layer: "coordination-plan".to_string(),
                task_id: Some(stem.to_string()),
                ..MemoryDocumentMetadata::default()
            };
        }
        if path.starts_with(".nanoclaw/memory/coordination/claims/") {
            return MemoryDocumentMetadata {
                scope: MemoryScope::Coordination,
                layer: "coordination-claim".to_string(),
                task_id: Some(stem.to_string()),
                ..MemoryDocumentMetadata::default()
            };
        }
        if path.starts_with(".nanoclaw/memory/coordination/handoffs/") {
            return MemoryDocumentMetadata {
                scope: MemoryScope::Coordination,
                layer: "coordination-handoff".to_string(),
                task_id: Some(stem.to_string()),
                ..MemoryDocumentMetadata::default()
            };
        }
    }

    if is_daily_log_path(path) {
        return MemoryDocumentMetadata {
            scope: MemoryScope::Episodic,
            layer: "daily-log".to_string(),
            ..MemoryDocumentMetadata::default()
        };
    }

    if path.starts_with("memory/") {
        return MemoryDocumentMetadata {
            scope: MemoryScope::Semantic,
            layer: "workspace-note".to_string(),
            ..MemoryDocumentMetadata::default()
        };
    }

    MemoryDocumentMetadata {
        scope: MemoryScope::Semantic,
        layer: "workspace-extra".to_string(),
        ..MemoryDocumentMetadata::default()
    }
}

fn merge_metadata(
    mut inferred: MemoryDocumentMetadata,
    frontmatter: Option<MemoryFrontmatter>,
    modified_at_ms: Option<u64>,
) -> MemoryDocumentMetadata {
    if let Some(frontmatter) = frontmatter {
        if let Some(scope) = frontmatter.scope {
            inferred.scope = scope;
        }
        if let Some(layer) = frontmatter.layer {
            let layer = layer.trim();
            if !layer.is_empty() {
                inferred.layer = layer.to_string();
            }
        }
        if let Some(run_id) = frontmatter.run_id {
            inferred.run_id = Some(run_id);
        }
        if let Some(session_id) = frontmatter.session_id {
            inferred.session_id = Some(session_id);
        }
        if let Some(agent_name) = normalize_optional_string(frontmatter.agent_name) {
            inferred.agent_name = Some(agent_name);
        }
        if let Some(task_id) = normalize_optional_string(frontmatter.task_id) {
            inferred.task_id = Some(task_id);
        }
        if let Some(updated_at_ms) = frontmatter.updated_at_ms {
            inferred.updated_at_ms = Some(updated_at_ms);
        }
        if let Some(promoted_from) = normalize_optional_string(frontmatter.promoted_from) {
            inferred.promoted_from = Some(promoted_from);
        }
        if let Some(supersedes) = frontmatter.supersedes {
            inferred.supersedes = normalize_string_list(supersedes);
        }
        if let Some(tags) = frontmatter.tags {
            inferred.tags = normalize_string_list(tags);
        }
        if let Some(status) = frontmatter.status {
            inferred.status = status;
        }
    }

    if inferred.updated_at_ms.is_none() {
        inferred.updated_at_ms = modified_at_ms;
    }
    inferred
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn normalize_string_list(values: Vec<String>) -> Vec<String> {
    let mut normalized = values
        .into_iter()
        .filter_map(|value| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    normalized
}

async fn load_file_fingerprint(path: &Path) -> Result<Option<MemoryCorpusFileFingerprint>> {
    let metadata = match fs::metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() {
        return Ok(None);
    }
    let modified = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok());
    Ok(Some(MemoryCorpusFileFingerprint {
        len: metadata.len(),
        modified_at_ms: modified
            .as_ref()
            .map(|duration| capped_duration_millis(*duration)),
        modified_at_ns: modified.map(capped_duration_nanos),
    }))
}

fn capped_duration_millis(duration: std::time::Duration) -> u64 {
    duration
        .as_millis()
        .min(u128::from(u64::MAX))
        .try_into()
        .unwrap_or(u64::MAX)
}

fn capped_duration_nanos(duration: std::time::Duration) -> u64 {
    duration
        .as_nanos()
        .min(u128::from(u64::MAX))
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
fn record_corpus_disk_read() {
    CORPUS_DISK_READS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(not(test))]
fn record_corpus_disk_read() {}

#[cfg(test)]
fn corpus_disk_read_count() -> usize {
    CORPUS_DISK_READS.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
fn reset_corpus_disk_read_count() {
    CORPUS_DISK_READS.store(0, std::sync::atomic::Ordering::Relaxed);
}

fn extract_title(lines: &[String], body_start_line: usize, path: &str) -> String {
    for line in lines.iter().skip(body_start_line.saturating_sub(1)) {
        let trimmed = line.trim();
        if let Some(title) = trimmed.strip_prefix("# ") {
            let title = title.trim();
            if !title.is_empty() {
                return title.to_string();
            }
        }
    }

    file_stem(path)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.to_string())
}

fn file_stem(path: &str) -> Option<&str> {
    path.rsplit('/').next()?.strip_suffix(".md")
}

fn is_daily_log_path(path: &str) -> bool {
    let Some(stem) = file_stem(path) else {
        return false;
    };
    let mut parts = stem.split('-');
    let Some(year) = parts.next() else {
        return false;
    };
    let Some(month) = parts.next() else {
        return false;
    };
    let Some(day) = parts.next() else {
        return false;
    };
    parts.next().is_none()
        && year.len() == 4
        && month.len() == 2
        && day.len() == 2
        && year.chars().all(|value| value.is_ascii_digit())
        && month.chars().all(|value| value.is_ascii_digit())
        && day.chars().all(|value| value.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::{
        chunk_corpus, corpus_disk_read_count, load_memory_corpus, reset_corpus_disk_read_count,
    };
    use crate::{MemoryChunkingConfig, MemoryCorpusConfig, MemoryScope};
    use std::time::Duration;
    use tempfile::tempdir;
    use tokio::{fs, time::sleep};

    #[tokio::test]
    async fn loads_default_memory_globs_only() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("MEMORY.md"), "global\nnote")
            .await
            .unwrap();
        fs::create_dir_all(dir.path().join("memory/2026"))
            .await
            .unwrap();
        fs::write(
            dir.path().join("memory/2026").join("today.md"),
            "today\nline two",
        )
        .await
        .unwrap();
        fs::write(dir.path().join("README.md"), "not memory")
            .await
            .unwrap();

        let corpus = load_memory_corpus(dir.path(), &MemoryCorpusConfig::default())
            .await
            .unwrap();
        assert_eq!(corpus.documents.len(), 2);
        assert_eq!(corpus.documents[0].path, "MEMORY.md");
        assert_eq!(corpus.documents[1].path, "memory/2026/today.md");
    }

    #[tokio::test]
    async fn includes_extra_markdown_paths() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("MEMORY.md"), "base")
            .await
            .unwrap();
        fs::create_dir_all(dir.path().join("notes")).await.unwrap();
        fs::write(dir.path().join("notes").join("design.md"), "extra")
            .await
            .unwrap();
        let config = MemoryCorpusConfig {
            extra_paths: vec![std::path::PathBuf::from("notes/design.md")],
            ..MemoryCorpusConfig::default()
        };

        let corpus = load_memory_corpus(dir.path(), &config).await.unwrap();
        assert_eq!(corpus.documents.len(), 2);
        assert!(
            corpus
                .documents
                .iter()
                .any(|doc| doc.path == "notes/design.md")
        );
    }

    #[tokio::test]
    async fn classifies_scopes_from_paths_and_frontmatter() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("MEMORY.md"), "# Root\nworkspace fact")
            .await
            .unwrap();
        fs::create_dir_all(dir.path().join(".nanoclaw/memory/working/sessions"))
            .await
            .unwrap();
        fs::write(
            dir.path()
                .join(".nanoclaw/memory/working/sessions/session_1.md"),
            "# Scratch\nactive task",
        )
        .await
        .unwrap();
        fs::create_dir_all(dir.path().join("memory")).await.unwrap();
        fs::write(
            dir.path().join("memory/howto.md"),
            "---\nscope: procedural\ntags:\n  - deploy\n---\n# Deploy\nrunbook",
        )
        .await
        .unwrap();
        fs::write(dir.path().join("memory/2026-03-28.md"), "# Log\nincident")
            .await
            .unwrap();

        let corpus = load_memory_corpus(dir.path(), &MemoryCorpusConfig::default())
            .await
            .unwrap();
        let by_path = corpus
            .documents
            .iter()
            .map(|doc| (doc.path.as_str(), doc))
            .collect::<std::collections::BTreeMap<_, _>>();

        assert_eq!(by_path["MEMORY.md"].metadata.scope, MemoryScope::Semantic);
        assert_eq!(
            by_path["memory/howto.md"].metadata.scope,
            MemoryScope::Procedural
        );
        assert_eq!(
            by_path["memory/2026-03-28.md"].metadata.scope,
            MemoryScope::Episodic
        );
        assert_eq!(
            by_path[".nanoclaw/memory/working/sessions/session_1.md"]
                .metadata
                .scope,
            MemoryScope::Working
        );
        assert_eq!(
            by_path["memory/howto.md"].metadata.tags,
            vec!["deploy".to_string()]
        );
    }

    #[tokio::test]
    async fn reuses_unchanged_documents_without_re_reading_disk() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("MEMORY.md"), "# Root\nfact")
            .await
            .unwrap();
        fs::create_dir_all(dir.path().join("memory")).await.unwrap();
        fs::write(dir.path().join("memory/cache.md"), "# Cache\nentry")
            .await
            .unwrap();

        reset_corpus_disk_read_count();
        let first = load_memory_corpus(dir.path(), &MemoryCorpusConfig::default())
            .await
            .unwrap();
        let first_reads = corpus_disk_read_count();
        let second = load_memory_corpus(dir.path(), &MemoryCorpusConfig::default())
            .await
            .unwrap();

        assert_eq!(first.documents, second.documents);
        assert_eq!(first_reads, 2);
        assert_eq!(corpus_disk_read_count(), first_reads);
    }

    #[tokio::test]
    async fn reloads_only_documents_whose_fingerprint_changed() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("MEMORY.md"), "# Root\nfact")
            .await
            .unwrap();
        fs::create_dir_all(dir.path().join("memory")).await.unwrap();
        fs::write(dir.path().join("memory/cache.md"), "# Cache\nentry")
            .await
            .unwrap();

        reset_corpus_disk_read_count();
        load_memory_corpus(dir.path(), &MemoryCorpusConfig::default())
            .await
            .unwrap();
        let baseline_reads = corpus_disk_read_count();

        sleep(Duration::from_millis(2)).await;
        fs::write(dir.path().join("memory/cache.md"), "# Cache\nupdated entry")
            .await
            .unwrap();
        let corpus = load_memory_corpus(dir.path(), &MemoryCorpusConfig::default())
            .await
            .unwrap();

        assert_eq!(corpus.documents.len(), 2);
        assert_eq!(corpus.documents[1].path, "memory/cache.md");
        assert_eq!(corpus_disk_read_count(), baseline_reads + 1);
    }

    #[tokio::test]
    async fn chunks_corpus_into_overlapping_windows() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("MEMORY.md"),
            "line1\nline2\nline3\nline4\nline5\nline6",
        )
        .await
        .unwrap();

        let corpus = load_memory_corpus(dir.path(), &MemoryCorpusConfig::default())
            .await
            .unwrap();
        let chunks = chunk_corpus(
            &corpus,
            &MemoryChunkingConfig {
                target_tokens: 2,
                overlap_tokens: 1,
            },
        );

        assert!(chunks.len() >= 2);
        assert_eq!(chunks[0].path, "MEMORY.md");
        assert!(chunks[0].end_line >= chunks[0].start_line);
        assert!(chunks[1].start_line <= chunks[0].end_line);
        assert_eq!(chunks[0].metadata.scope, MemoryScope::Semantic);
    }

    #[tokio::test]
    async fn chunks_corpus_advances_when_target_only_fits_one_line() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("MEMORY.md"),
            [
                "semantic line one",
                "semantic line two",
                "semantic line three",
                "semantic line four",
            ]
            .join("\n"),
        )
        .await
        .unwrap();

        let corpus = load_memory_corpus(dir.path(), &MemoryCorpusConfig::default())
            .await
            .unwrap();
        let chunks = chunk_corpus(
            &corpus,
            &MemoryChunkingConfig {
                target_tokens: 8,
                overlap_tokens: 1,
            },
        );

        assert_eq!(
            chunks
                .iter()
                .map(|chunk| (chunk.start_line, chunk.end_line))
                .collect::<Vec<_>>(),
            vec![(1, 1), (2, 2), (3, 3), (4, 4)]
        );
    }
}
