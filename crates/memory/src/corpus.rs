use crate::{MemoryChunkingConfig, MemoryCorpusConfig, MemoryError, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::fs;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryCorpusDocument {
    pub path: String,
    pub absolute_path: PathBuf,
    pub snapshot_id: String,
    pub lines: Vec<String>,
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
}

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
            });

            if end >= document.lines.len() {
                break;
            }

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
            start = overlap_start.min(end.saturating_sub(1));
        }
    }

    chunks
}

pub async fn load_memory_corpus(
    workspace_root: &Path,
    config: &MemoryCorpusConfig,
) -> Result<MemoryCorpus> {
    let include_set = build_globset(&config.include_globs)?;
    let mut candidates = discover_default_candidates(workspace_root, &include_set)?;
    candidates.extend(resolve_extra_paths(workspace_root, &config.extra_paths)?);
    candidates.sort();
    candidates.dedup();

    let mut documents = Vec::new();
    for absolute_path in candidates {
        if !absolute_path.exists() || !absolute_path.is_file() {
            continue;
        }
        let relative = absolute_path
            .strip_prefix(workspace_root)
            .map_err(|_| MemoryError::PathOutsideWorkspace(absolute_path.display().to_string()))?;
        let relative_path = normalize_relative_path(relative);
        if !include_set.is_match(relative) && !is_extra_path(config, relative) {
            continue;
        }
        let text = fs::read_to_string(&absolute_path).await?;
        let lines = parse_lines(&text);
        documents.push(MemoryCorpusDocument {
            path: relative_path,
            absolute_path,
            snapshot_id: stable_hash(&text),
            lines,
        });
    }

    documents.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(MemoryCorpus { documents })
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

fn stable_hash(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn is_extra_path(config: &MemoryCorpusConfig, relative: &Path) -> bool {
    config
        .extra_paths
        .iter()
        .filter(|path| !path.is_absolute())
        .any(|path| relative == path)
}

#[cfg(test)]
mod tests {
    use super::{chunk_corpus, load_memory_corpus};
    use crate::{MemoryChunkingConfig, MemoryCorpusConfig};
    use tempfile::tempdir;
    use tokio::fs;

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
    }
}
