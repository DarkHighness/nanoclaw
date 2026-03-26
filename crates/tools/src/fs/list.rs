use crate::annotations::mcp_tool_annotations;
use crate::fs::resolve_tool_path_against_workspace_root;
use crate::registry::Tool;
use crate::{Result, ToolExecutionContext};
use async_trait::async_trait;
use ignore::WalkBuilder;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;
use types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};

const DEFAULT_LIST_LIMIT: usize = 200;
const DEFAULT_LIST_MAX_DEPTH: usize = 4;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct ListToolInput {
    pub path: Option<String>,
    pub recursive: Option<bool>,
    pub max_depth: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct ListTool;

impl ListTool {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum EntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

impl EntryKind {
    fn marker(&self) -> &'static str {
        match self {
            Self::File => "F",
            Self::Directory => "D",
            Self::Symlink => "L",
            Self::Other => "O",
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
            Self::Symlink => "symlink",
            Self::Other => "other",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ListEntry {
    path: String,
    kind: EntryKind,
}

#[async_trait]
impl Tool for ListTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list".into(),
            description: "List files and directories under a workspace path. Respects ignore files and supports bounded recursive listing.".to_string(),
            input_schema: serde_json::to_value(schema_for!(ListToolInput)).expect("list schema"),
            output_mode: ToolOutputMode::Text,
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("List Files", true, false, true, false),
        }
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: ListToolInput = serde_json::from_value(arguments)?;
        let recursive = input.recursive.unwrap_or(false);
        let max_depth = input
            .max_depth
            .unwrap_or(if recursive { DEFAULT_LIST_MAX_DEPTH } else { 1 })
            .max(1);
        let limit = input.limit.unwrap_or(DEFAULT_LIST_LIMIT).max(1);
        let requested_path = input.path.as_deref().unwrap_or(".");

        let root = resolve_tool_path_against_workspace_root(
            requested_path,
            ctx.effective_root(),
            ctx.container_workdir.as_deref(),
        )?;
        if ctx.workspace_only {
            ctx.assert_path_allowed(&root)?;
        }

        let mut entries = if root.is_file() {
            let path = normalize_requested_file_path(requested_path, &root);
            vec![ListEntry {
                path,
                kind: EntryKind::File,
            }]
        } else {
            collect_entries(&root, recursive, max_depth, limit + 1)?
        };
        entries.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| kind_rank(&left.kind).cmp(&kind_rank(&right.kind)))
        });

        let truncated = entries.len() > limit;
        if truncated {
            entries.truncate(limit);
        }

        let file_count = entries
            .iter()
            .filter(|entry| matches!(entry.kind, EntryKind::File))
            .count();
        let dir_count = entries
            .iter()
            .filter(|entry| matches!(entry.kind, EntryKind::Directory))
            .count();
        let symlink_count = entries
            .iter()
            .filter(|entry| matches!(entry.kind, EntryKind::Symlink))
            .count();
        let other_count = entries
            .iter()
            .filter(|entry| matches!(entry.kind, EntryKind::Other))
            .count();

        let header = format!(
            "[list path={} recursive={} max_depth={} entries={} truncated={}]",
            requested_path,
            recursive,
            max_depth,
            entries.len(),
            truncated
        );

        let mut output_lines = vec![header.clone()];
        if entries.is_empty() {
            output_lines.push("[No entries found]".to_string());
        } else {
            output_lines.extend(
                entries
                    .iter()
                    .map(|entry| format!("[{}] {}", entry.kind.marker(), entry.path)),
            );
        }
        if truncated {
            output_lines.push(format!(
                "[{limit} entries limit reached. Narrow the path or increase limit.]"
            ));
        }

        let encoded_entries: Vec<Value> = entries
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "path": entry.path,
                    "kind": entry.kind.as_str(),
                })
            })
            .collect();

        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "list".into(),
            parts: vec![MessagePart::text(output_lines.join("\n"))],
            metadata: Some(serde_json::json!({
                "path": root,
                "requested_path": requested_path,
                "recursive": recursive,
                "max_depth": max_depth,
                "limit": limit,
                "entry_count": entries.len(),
                "truncated": truncated,
                "header": header,
                "entries": encoded_entries,
                "counts": {
                    "files": file_count,
                    "directories": dir_count,
                    "symlinks": symlink_count,
                    "other": other_count
                }
            })),
            is_error: false,
        })
    }
}

fn kind_rank(kind: &EntryKind) -> u8 {
    match kind {
        EntryKind::Directory => 0,
        EntryKind::File => 1,
        EntryKind::Symlink => 2,
        EntryKind::Other => 3,
    }
}

fn collect_entries(
    root: &Path,
    recursive: bool,
    max_depth: usize,
    limit: usize,
) -> Result<Vec<ListEntry>> {
    let mut builder = WalkBuilder::new(root);
    builder.hidden(false);
    builder.git_ignore(true);
    builder.git_global(true);
    builder.git_exclude(true);
    builder.require_git(false);
    builder.max_depth(Some(if recursive { max_depth } else { 1 }));
    builder.follow_links(false);

    let mut entries = Vec::new();
    for entry in builder.build() {
        let entry = entry?;
        let path = entry.path();
        if path == root {
            continue;
        }

        let relative = path.strip_prefix(root).unwrap_or(path);
        let metadata = entry.metadata()?;
        let file_type = metadata.file_type();
        let kind = if file_type.is_symlink() {
            EntryKind::Symlink
        } else if file_type.is_dir() {
            EntryKind::Directory
        } else if file_type.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        };

        entries.push(ListEntry {
            path: relative.to_string_lossy().replace('\\', "/"),
            kind,
        });

        if entries.len() >= limit {
            break;
        }
    }
    Ok(entries)
}

fn normalize_requested_file_path(requested_path: &str, root: &Path) -> String {
    if requested_path == "." {
        return root
            .file_name()
            .and_then(|value| value.to_str())
            .map_or_else(|| root.to_string_lossy().to_string(), ToOwned::to_owned);
    }
    requested_path
        .strip_prefix("./")
        .unwrap_or(requested_path)
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::{ListTool, ListToolInput};
    use crate::{Tool, ToolExecutionContext};
    use std::path::PathBuf;
    use types::ToolCallId;

    fn context(root: PathBuf) -> ToolExecutionContext {
        ToolExecutionContext {
            workspace_root: root,
            workspace_only: true,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn list_tool_lists_shallow_entries_by_default() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(dir.path().join("src"))
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("README.md"), "hi")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("src/lib.rs"), "")
            .await
            .unwrap();

        let result = ListTool::new()
            .execute(
                ToolCallId::new(),
                serde_json::to_value(ListToolInput {
                    path: None,
                    recursive: None,
                    max_depth: None,
                    limit: None,
                })
                .unwrap(),
                &context(dir.path().to_path_buf()),
            )
            .await
            .unwrap();

        let output = result.text_content();
        assert!(output.contains("[D] src"));
        assert!(output.contains("[F] README.md"));
        assert!(!output.contains("src/lib.rs"));
    }

    #[tokio::test]
    async fn list_tool_respects_gitignore_by_default() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(dir.path().join("build"))
            .await
            .unwrap();
        tokio::fs::write(dir.path().join(".gitignore"), "build/\n")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("build/output.log"), "x")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("visible.txt"), "v")
            .await
            .unwrap();

        let result = ListTool::new()
            .execute(
                ToolCallId::new(),
                serde_json::to_value(ListToolInput {
                    path: None,
                    recursive: Some(true),
                    max_depth: Some(4),
                    limit: None,
                })
                .unwrap(),
                &context(dir.path().to_path_buf()),
            )
            .await
            .unwrap();

        let output = result.text_content();
        assert!(output.contains("visible.txt"));
        assert!(!output.contains("build"));
        assert!(!output.contains("output.log"));
    }

    #[tokio::test]
    async fn list_tool_applies_limit_and_reports_truncation() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(dir.path().join("src"))
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("a.txt"), "")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("b.txt"), "")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("src/c.txt"), "")
            .await
            .unwrap();

        let result = ListTool::new()
            .execute(
                ToolCallId::new(),
                serde_json::to_value(ListToolInput {
                    path: None,
                    recursive: Some(true),
                    max_depth: Some(4),
                    limit: Some(2),
                })
                .unwrap(),
                &context(dir.path().to_path_buf()),
            )
            .await
            .unwrap();

        let output = result.text_content();
        assert!(output.contains("limit reached"));
        let metadata = result.metadata.unwrap();
        assert_eq!(metadata["truncated"], true);
        assert_eq!(metadata["entry_count"], 2);
        assert_eq!(metadata["limit"].as_u64().unwrap(), 2);
        let entries = metadata["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert!(
            metadata["header"]
                .as_str()
                .unwrap()
                .starts_with("[list path=")
        );
        assert_eq!(metadata["counts"]["files"].as_u64().unwrap(), 2);
    }

    #[tokio::test]
    async fn list_tool_preserves_requested_file_path() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(dir.path().join("src"))
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("src/lib.rs"), "pub fn f() {}")
            .await
            .unwrap();

        let result = ListTool::new()
            .execute(
                ToolCallId::new(),
                serde_json::to_value(ListToolInput {
                    path: Some("src/lib.rs".to_string()),
                    recursive: None,
                    max_depth: None,
                    limit: None,
                })
                .unwrap(),
                &context(dir.path().to_path_buf()),
            )
            .await
            .unwrap();

        let output = result.text_content();
        assert!(output.contains("[F] src/lib.rs"));
    }
}
