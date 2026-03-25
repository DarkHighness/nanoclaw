use crate::fs::{TextBuffer, stable_text_hash};
use crate::{Result, ToolError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;
use tokio::fs;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WriteExistingBehavior {
    Error,
    #[default]
    Overwrite,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WriteMissingBehavior {
    #[default]
    Create,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum TextEditOperation {
    StrReplace {
        old_text: String,
        new_text: String,
        #[serde(default)]
        replace_all: bool,
    },
    ReplaceLines {
        start_line: usize,
        end_line: usize,
        text: String,
        #[serde(default)]
        expected_selection_hash: Option<String>,
    },
    Insert {
        #[serde(alias = "insert_line")]
        after_line: usize,
        text: String,
    },
}

#[derive(Clone, Debug)]
pub struct MutationOutcome {
    pub next_content: Option<String>,
    pub summary: String,
    pub metadata: Value,
    pub is_error: bool,
    pub snapshot_before: Option<String>,
    pub snapshot_after: Option<String>,
}

#[derive(Clone, Debug)]
pub struct WriteRequest {
    pub content: String,
    pub if_exists: WriteExistingBehavior,
    pub if_missing: WriteMissingBehavior,
    pub expected_snapshot: Option<String>,
}

pub async fn load_optional_text_file(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path).await {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub async fn commit_text_file(path: &Path, content: Option<&str>) -> Result<()> {
    match content {
        Some(content) => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).await?;
            }
            fs::write(path, content.as_bytes()).await?;
        }
        None => {
            if fs::try_exists(path).await? {
                fs::remove_file(path).await?;
            }
        }
    }
    Ok(())
}

pub fn apply_write(existing: Option<&str>, path: &str, request: &WriteRequest) -> MutationOutcome {
    let snapshot_before = existing.map(stable_text_hash);
    if let Some(expected_snapshot) = request.expected_snapshot.as_deref()
        && snapshot_before.as_deref() != Some(expected_snapshot)
    {
        return error_outcome(
            snapshot_before,
            format!(
                "Snapshot mismatch for {path}. Expected {expected_snapshot}, re-read before overwriting."
            ),
            json!({
                "command": "write",
                "path": path,
                "expected_snapshot": expected_snapshot,
            }),
        );
    }

    match existing {
        Some(_) if matches!(request.if_exists, WriteExistingBehavior::Error) => error_outcome(
            snapshot_before,
            format!(
                "{path} already exists. Re-run with if_exists=overwrite or use edit/patch for partial changes."
            ),
            json!({
                "command": "write",
                "path": path,
                "if_exists": request.if_exists,
            }),
        ),
        None if matches!(request.if_missing, WriteMissingBehavior::Error) => error_outcome(
            snapshot_before,
            format!("{path} does not exist. Re-run with if_missing=create."),
            json!({
                "command": "write",
                "path": path,
                "if_missing": request.if_missing,
            }),
        ),
        _ => {
            let snapshot_after = stable_text_hash(&request.content);
            MutationOutcome {
                next_content: Some(request.content.clone()),
                summary: if existing.is_some() {
                    format!("Wrote {} bytes to {path}", request.content.len())
                } else {
                    format!("Created {path} with {} bytes", request.content.len())
                },
                metadata: json!({
                    "command": "write",
                    "path": path,
                    "created": existing.is_none(),
                    "bytes_written": request.content.len(),
                    "if_exists": request.if_exists,
                    "if_missing": request.if_missing,
                    "used_expected_snapshot": request.expected_snapshot.is_some(),
                }),
                is_error: false,
                snapshot_before,
                snapshot_after: Some(snapshot_after),
            }
        }
    }
}

pub fn apply_text_edits(
    existing: Option<&str>,
    path: &str,
    expected_snapshot: Option<&str>,
    edits: &[TextEditOperation],
) -> Result<MutationOutcome> {
    let Some(existing) = existing else {
        return Ok(error_outcome(
            None,
            format!("{path} does not exist. Use write or patch/write to create it first."),
            json!({
                "command": "edit",
                "path": path,
            }),
        ));
    };
    if edits.is_empty() {
        return Err(ToolError::invalid("edit requires at least one operation"));
    }

    let snapshot_before = stable_text_hash(existing);
    if let Some(expected_snapshot) = expected_snapshot
        && expected_snapshot != snapshot_before
    {
        let found_snapshot = snapshot_before.clone();
        return Ok(error_outcome(
            Some(snapshot_before),
            format!(
                "Snapshot mismatch for {path}. Expected {expected_snapshot}, found {found_snapshot}. Re-read the file before editing."
            ),
            json!({
                "command": "edit",
                "path": path,
                "expected_snapshot": expected_snapshot,
            }),
        ));
    }

    let mut working = existing.to_string();
    let mut operations = Vec::with_capacity(edits.len());
    for operation in edits {
        let outcome = apply_single_edit(path, &working, operation)?;
        if outcome.is_error {
            return Ok(MutationOutcome {
                next_content: Some(working),
                summary: outcome.summary,
                metadata: json!({
                    "command": "edit",
                    "path": path,
                    "operations": operations,
                    "failed_operation": outcome.metadata,
                }),
                is_error: true,
                snapshot_before: Some(snapshot_before),
                snapshot_after: None,
            });
        }
        working = outcome.next_content.ok_or_else(|| {
            ToolError::invalid_state("text edit unexpectedly removed file content")
        })?;
        operations.push(outcome.metadata);
    }

    let snapshot_after = stable_text_hash(&working);
    Ok(MutationOutcome {
        next_content: Some(working),
        summary: format!("Edited {path} with {} operation(s)", edits.len()),
        metadata: json!({
            "command": "edit",
            "path": path,
            "operations": operations,
        }),
        is_error: false,
        snapshot_before: Some(snapshot_before),
        snapshot_after: Some(snapshot_after),
    })
}

pub fn apply_delete(
    existing: Option<&str>,
    path: &str,
    expected_snapshot: Option<&str>,
    ignore_missing: bool,
) -> MutationOutcome {
    let snapshot_before = existing.map(stable_text_hash);
    match existing {
        None if ignore_missing => MutationOutcome {
            next_content: None,
            summary: format!("Skipped delete for missing file {path}"),
            metadata: json!({
                "command": "delete",
                "path": path,
                "deleted": false,
                "ignore_missing": true,
            }),
            is_error: false,
            snapshot_before: None,
            snapshot_after: None,
        },
        None => error_outcome(
            None,
            format!(
                "{path} does not exist. Re-run with ignore_missing=true to treat it as a no-op."
            ),
            json!({
                "command": "delete",
                "path": path,
                "ignore_missing": false,
            }),
        ),
        Some(_) => {
            if let Some(expected_snapshot) = expected_snapshot
                && snapshot_before.as_deref() != Some(expected_snapshot)
            {
                return error_outcome(
                    snapshot_before,
                    format!(
                        "Snapshot mismatch for {path}. Expected {expected_snapshot}, re-read before deleting."
                    ),
                    json!({
                        "command": "delete",
                        "path": path,
                        "expected_snapshot": expected_snapshot,
                    }),
                );
            }
            MutationOutcome {
                next_content: None,
                summary: format!("Deleted {path}"),
                metadata: json!({
                    "command": "delete",
                    "path": path,
                    "deleted": true,
                    "ignore_missing": ignore_missing,
                }),
                is_error: false,
                snapshot_before,
                snapshot_after: None,
            }
        }
    }
}

fn apply_single_edit(
    path: &str,
    content: &str,
    operation: &TextEditOperation,
) -> Result<MutationOutcome> {
    match operation {
        TextEditOperation::StrReplace {
            old_text,
            new_text,
            replace_all,
        } => run_string_replace(path, content, old_text, new_text, *replace_all),
        TextEditOperation::ReplaceLines {
            start_line,
            end_line,
            text,
            expected_selection_hash,
        } => run_line_replace(
            path,
            content,
            *start_line,
            *end_line,
            text,
            expected_selection_hash.as_deref(),
        ),
        TextEditOperation::Insert { after_line, text } => {
            run_insert(path, content, *after_line, text)
        }
    }
}

fn run_string_replace(
    path: &str,
    content: &str,
    old_text: &str,
    new_text: &str,
    replace_all: bool,
) -> Result<MutationOutcome> {
    let occurrences = content.matches(old_text).count();
    if occurrences == 0 {
        return Ok(error_outcome(
            Some(stable_text_hash(content)),
            format!("No exact match found in {path}"),
            json!({
                "command": "str_replace",
                "path": path,
                "occurrences": 0,
                "replace_all": replace_all,
            }),
        ));
    }
    if occurrences > 1 && !replace_all {
        return Ok(error_outcome(
            Some(stable_text_hash(content)),
            format!(
                "Found {occurrences} matches in {path}. Re-run with replace_all=true or provide a more specific old_text."
            ),
            json!({
                "command": "str_replace",
                "path": path,
                "occurrences": occurrences,
                "replace_all": false,
            }),
        ));
    }

    let updated = if replace_all {
        content.replace(old_text, new_text)
    } else {
        content.replacen(old_text, new_text, 1)
    };
    Ok(MutationOutcome {
        next_content: Some(updated),
        summary: format!(
            "Edited {path} using str_replace ({} replacement{})",
            if replace_all { occurrences } else { 1 },
            if replace_all && occurrences != 1 {
                "s"
            } else {
                ""
            }
        ),
        metadata: json!({
            "command": "str_replace",
            "path": path,
            "occurrences": occurrences,
            "replace_all": replace_all,
        }),
        is_error: false,
        snapshot_before: Some(stable_text_hash(content)),
        snapshot_after: None,
    })
}

fn run_line_replace(
    path: &str,
    content: &str,
    start_line: usize,
    end_line: usize,
    replacement_text: &str,
    expected_selection_hash: Option<&str>,
) -> Result<MutationOutcome> {
    let mut buffer = TextBuffer::parse(content);
    let current_slice = buffer.line_slice_text(start_line, end_line)?;
    let current_slice_hash = stable_text_hash(&current_slice);
    if let Some(expected_selection_hash) = expected_selection_hash
        && expected_selection_hash != current_slice_hash
    {
        return Ok(error_outcome(
            Some(stable_text_hash(content)),
            format!(
                "Slice mismatch for {path} lines {start_line}-{end_line}. Expected {expected_selection_hash}, found {current_slice_hash}. Re-read that range before editing."
            ),
            json!({
                "command": "replace_lines",
                "path": path,
                "start_line": start_line,
                "end_line": end_line,
                "current_slice_hash": current_slice_hash,
            }),
        ));
    }

    buffer.replace_lines(start_line, end_line, replacement_text)?;
    Ok(MutationOutcome {
        next_content: Some(buffer.to_text()),
        summary: format!("Edited {path} using replace_lines ({start_line}-{end_line})"),
        metadata: json!({
            "command": "replace_lines",
            "path": path,
            "start_line": start_line,
            "end_line": end_line,
            "previous_slice_hash": current_slice_hash,
        }),
        is_error: false,
        snapshot_before: Some(stable_text_hash(content)),
        snapshot_after: None,
    })
}

fn run_insert(
    path: &str,
    content: &str,
    after_line: usize,
    insert_text: &str,
) -> Result<MutationOutcome> {
    let mut buffer = TextBuffer::parse(content);
    buffer.insert_after(after_line, insert_text)?;
    Ok(MutationOutcome {
        next_content: Some(buffer.to_text()),
        summary: format!("Edited {path} using insert (after line {after_line})"),
        metadata: json!({
            "command": "insert",
            "path": path,
            "after_line": after_line,
        }),
        is_error: false,
        snapshot_before: Some(stable_text_hash(content)),
        snapshot_after: None,
    })
}

fn error_outcome(
    snapshot_before: Option<String>,
    summary: String,
    metadata: Value,
) -> MutationOutcome {
    MutationOutcome {
        next_content: None,
        summary,
        metadata,
        is_error: true,
        snapshot_before,
        snapshot_after: None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TextEditOperation, WriteExistingBehavior, WriteMissingBehavior, WriteRequest, apply_delete,
        apply_text_edits, apply_write,
    };

    #[test]
    fn write_can_create_new_files() {
        let outcome = apply_write(
            None,
            "sample.txt",
            &WriteRequest {
                content: "hello\n".to_string(),
                if_exists: WriteExistingBehavior::Overwrite,
                if_missing: WriteMissingBehavior::Create,
                expected_snapshot: None,
            },
        );
        assert!(!outcome.is_error);
        assert_eq!(outcome.next_content.as_deref(), Some("hello\n"));
    }

    #[test]
    fn edit_batch_applies_multiple_operations() {
        let outcome = apply_text_edits(
            Some("alpha\nbeta\ngamma\n"),
            "sample.txt",
            None,
            &[
                TextEditOperation::ReplaceLines {
                    start_line: 2,
                    end_line: 2,
                    text: "middle".to_string(),
                    expected_selection_hash: None,
                },
                TextEditOperation::Insert {
                    after_line: 3,
                    text: "tail".to_string(),
                },
            ],
        )
        .unwrap();
        assert!(!outcome.is_error);
        assert_eq!(
            outcome.next_content.as_deref(),
            Some("alpha\nmiddle\ngamma\ntail\n")
        );
    }

    #[test]
    fn delete_can_skip_missing_files() {
        let outcome = apply_delete(None, "sample.txt", None, true);
        assert!(!outcome.is_error);
        assert!(outcome.next_content.is_none());
    }
}
