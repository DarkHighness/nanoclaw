use crate::ToolExecutionContext;
use crate::fs::patch::PatchOperation;
use crate::fs::{
    WriteExistingBehavior, WriteMissingBehavior, load_optional_text_file,
    resolve_tool_path_against_workspace_root, stable_text_hash,
};
use crate::{Result, ToolError};

pub(crate) const PATCH_FILES_FREEFORM_LARK_GRAMMAR: &str = include_str!("patch_files.lark");

#[derive(Clone, Debug)]
enum PatchHunk {
    Add {
        path: String,
        contents: String,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_path: Option<String>,
        chunks: Vec<UpdateChunk>,
    },
}

#[derive(Clone, Debug)]
struct UpdateChunk {
    change_context: Option<String>,
    old_lines: Vec<String>,
    new_lines: Vec<String>,
    is_end_of_file: bool,
}

pub(crate) async fn parse_patch_files_operations(
    patch_text: &str,
    ctx: &ToolExecutionContext,
) -> Result<Vec<PatchOperation>> {
    let hunks = parse_patch_files(patch_text)?;
    build_patch_operations(&hunks, ctx).await
}

fn parse_patch_files(patch_text: &str) -> Result<Vec<PatchHunk>> {
    let normalized = patch_text.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = normalized.trim();
    let lines = trimmed.split('\n').collect::<Vec<_>>();
    let begin_index = lines
        .iter()
        .position(|line| *line == "*** Begin Patch")
        .ok_or_else(|| {
            ToolError::invalid("patch_files freeform input is missing `*** Begin Patch`")
        })?;
    let end_index = lines
        .iter()
        .rposition(|line| *line == "*** End Patch")
        .ok_or_else(|| {
            ToolError::invalid("patch_files freeform input is missing `*** End Patch`")
        })?;
    if begin_index >= end_index {
        return Err(ToolError::invalid(
            "patch_files freeform markers are malformed; `*** End Patch` must come after `*** Begin Patch`",
        ));
    }

    let mut hunks = Vec::new();
    let mut index = begin_index + 1;
    while index < end_index {
        let line = lines[index];
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            let path = parse_non_empty_path(path, "add file")?;
            let (contents, next_index) = parse_add_contents(&lines, index + 1, end_index)?;
            hunks.push(PatchHunk::Add { path, contents });
            index = next_index;
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            let path = parse_non_empty_path(path, "delete file")?;
            hunks.push(PatchHunk::Delete { path });
            index += 1;
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Update File: ") {
            let path = parse_non_empty_path(path, "update file")?;
            let mut next_index = index + 1;
            let move_path = if next_index < end_index {
                lines[next_index]
                    .strip_prefix("*** Move to: ")
                    .map(|value| {
                        next_index += 1;
                        parse_non_empty_path(value, "move target")
                    })
                    .transpose()?
            } else {
                None
            };
            let (chunks, parsed_index) = parse_update_chunks(&lines, next_index, end_index)?;
            if chunks.is_empty() && move_path.is_none() {
                return Err(ToolError::invalid(format!(
                    "update hunk for `{}` must include at least one change or a move target",
                    path
                )));
            }
            hunks.push(PatchHunk::Update {
                path,
                move_path,
                chunks,
            });
            index = parsed_index;
            continue;
        }
        if line.trim().is_empty() {
            index += 1;
            continue;
        }
        return Err(ToolError::invalid(format!(
            "unexpected patch_files freeform line `{line}`"
        )));
    }

    if hunks.is_empty() {
        return Err(ToolError::invalid(
            "patch_files freeform input requires at least one file hunk between the begin/end markers",
        ));
    }
    Ok(hunks)
}

fn parse_non_empty_path(raw: &str, label: &str) -> Result<String> {
    let path = raw.trim();
    if path.is_empty() {
        return Err(ToolError::invalid(format!(
            "patch_files freeform {label} path must not be empty"
        )));
    }
    Ok(path.to_string())
}

fn parse_add_contents(
    lines: &[&str],
    mut index: usize,
    end_index: usize,
) -> Result<(String, usize)> {
    let mut content_lines = Vec::new();
    while index < end_index && !lines[index].starts_with("***") {
        let line = lines[index];
        let content = line.strip_prefix('+').ok_or_else(|| {
            ToolError::invalid("patch_files freeform add hunks may only contain `+` lines")
        })?;
        content_lines.push(content.to_string());
        index += 1;
    }
    if content_lines.is_empty() {
        return Err(ToolError::invalid(
            "patch_files freeform add file hunks must contain at least one `+` line",
        ));
    }
    Ok((content_lines.join("\n"), index))
}

fn parse_update_chunks(
    lines: &[&str],
    mut index: usize,
    end_index: usize,
) -> Result<(Vec<UpdateChunk>, usize)> {
    let mut chunks = Vec::new();
    while index < end_index
        && (!lines[index].starts_with("***") || lines[index] == "*** End of File")
    {
        let line = lines[index];
        let Some(context) = line.strip_prefix("@@") else {
            return Err(ToolError::invalid(format!(
                "patch_files freeform update hunks must start each chunk with `@@`; found `{line}`"
            )));
        };
        index += 1;

        let mut old_lines = Vec::new();
        let mut new_lines = Vec::new();
        let mut is_end_of_file = false;
        while index < end_index
            && !lines[index].starts_with("@@")
            && (!lines[index].starts_with("***") || lines[index] == "*** End of File")
        {
            let change_line = lines[index];
            if change_line == "*** End of File" {
                is_end_of_file = true;
                index += 1;
                break;
            }
            if change_line.is_empty() {
                return Err(ToolError::invalid(
                    "patch_files freeform change lines must begin with ` `, `+`, or `-`",
                ));
            }
            let (prefix, content) = change_line.split_at(1);
            match prefix {
                " " => {
                    old_lines.push(content.to_string());
                    new_lines.push(content.to_string());
                }
                "-" => old_lines.push(content.to_string()),
                "+" => new_lines.push(content.to_string()),
                _ => {
                    return Err(ToolError::invalid(format!(
                        "unsupported patch_files freeform change line `{change_line}`"
                    )));
                }
            }
            index += 1;
        }

        chunks.push(UpdateChunk {
            change_context: (!context.trim().is_empty()).then(|| context.trim().to_string()),
            old_lines,
            new_lines,
            is_end_of_file,
        });
    }
    Ok((chunks, index))
}

async fn build_patch_operations(
    hunks: &[PatchHunk],
    ctx: &ToolExecutionContext,
) -> Result<Vec<PatchOperation>> {
    let mut operations = Vec::new();
    for hunk in hunks {
        match hunk {
            PatchHunk::Add { path, contents } => operations.push(PatchOperation::Write {
                path: path.clone(),
                content: contents.clone(),
                if_exists: Some(WriteExistingBehavior::Overwrite),
                if_missing: Some(WriteMissingBehavior::Create),
                expected_snapshot: None,
            }),
            PatchHunk::Delete { path } => {
                let snapshot = load_snapshot(path, ctx).await?;
                operations.push(PatchOperation::Delete {
                    path: path.clone(),
                    expected_snapshot: snapshot,
                    ignore_missing: Some(false),
                });
            }
            PatchHunk::Update {
                path,
                move_path,
                chunks,
            } => {
                let resolved = resolve_tool_path_against_workspace_root(
                    path,
                    ctx.effective_root(),
                    ctx.container_workdir.as_deref(),
                )?;
                ctx.assert_path_write_allowed(&resolved)?;
                let original_content =
                    load_optional_text_file(&resolved).await?.ok_or_else(|| {
                        ToolError::invalid(format!("cannot update missing file `{path}`"))
                    })?;
                let snapshot = stable_text_hash(&original_content);
                let updated_content = if chunks.is_empty() {
                    original_content.clone()
                } else {
                    derive_new_content(path, &original_content, chunks)?
                };

                if let Some(move_path) = move_path {
                    operations.push(PatchOperation::Move {
                        from_path: path.clone(),
                        to_path: move_path.clone(),
                        expected_snapshot: Some(snapshot.clone()),
                        if_destination_exists: Some(WriteExistingBehavior::Overwrite),
                        ignore_missing: Some(false),
                    });
                    if updated_content != original_content {
                        operations.push(PatchOperation::Write {
                            path: move_path.clone(),
                            content: updated_content,
                            if_exists: Some(WriteExistingBehavior::Overwrite),
                            if_missing: Some(WriteMissingBehavior::Error),
                            expected_snapshot: Some(snapshot),
                        });
                    }
                } else {
                    operations.push(PatchOperation::Write {
                        path: path.clone(),
                        content: updated_content,
                        if_exists: Some(WriteExistingBehavior::Overwrite),
                        if_missing: Some(WriteMissingBehavior::Error),
                        expected_snapshot: Some(snapshot),
                    });
                }
            }
        }
    }
    Ok(operations)
}

async fn load_snapshot(path: &str, ctx: &ToolExecutionContext) -> Result<Option<String>> {
    let resolved = resolve_tool_path_against_workspace_root(
        path,
        ctx.effective_root(),
        ctx.container_workdir.as_deref(),
    )?;
    ctx.assert_path_write_allowed(&resolved)?;
    Ok(load_optional_text_file(&resolved)
        .await?
        .map(|content| stable_text_hash(&content)))
}

fn derive_new_content(
    path: &str,
    original_content: &str,
    chunks: &[UpdateChunk],
) -> Result<String> {
    let mut original_lines = original_content
        .split('\n')
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if original_lines.last().is_some_and(|line| line.is_empty()) {
        original_lines.pop();
    }

    let replacements = compute_replacements(&original_lines, path, chunks)?;
    let mut new_lines = apply_replacements(&original_lines, &replacements);
    if new_lines.last().is_none_or(|line| !line.is_empty()) {
        new_lines.push(String::new());
    }
    Ok(new_lines.join("\n"))
}

type Replacement = (usize, usize, Vec<String>);

fn compute_replacements(
    original_lines: &[String],
    path: &str,
    chunks: &[UpdateChunk],
) -> Result<Vec<Replacement>> {
    let mut replacements = Vec::new();
    let mut line_index = 0;

    for chunk in chunks {
        if let Some(context) = &chunk.change_context {
            let Some(context_index) = seek_sequence(
                original_lines,
                std::slice::from_ref(context),
                line_index,
                false,
            ) else {
                return Err(ToolError::invalid(format!(
                    "failed to find context `{context}` in `{path}`"
                )));
            };
            line_index = context_index + 1;
        }

        if chunk.old_lines.is_empty() {
            let insertion_index = original_lines.len();
            replacements.push((insertion_index, 0, chunk.new_lines.clone()));
            continue;
        }

        let mut pattern = chunk.old_lines.clone();
        let mut replacement = chunk.new_lines.clone();
        let mut found = seek_sequence(original_lines, &pattern, line_index, chunk.is_end_of_file);
        if found.is_none() && pattern.last().is_some_and(|line| line.is_empty()) {
            pattern.pop();
            if replacement.last().is_some_and(|line| line.is_empty()) {
                replacement.pop();
            }
            found = seek_sequence(original_lines, &pattern, line_index, chunk.is_end_of_file);
        }

        let Some(found_index) = found else {
            return Err(ToolError::invalid(format!(
                "failed to find expected lines in `{path}`:\n{}",
                chunk.old_lines.join("\n")
            )));
        };
        line_index = found_index + pattern.len();
        replacements.push((found_index, pattern.len(), replacement));
    }

    replacements.sort_by_key(|entry| entry.0);
    Ok(replacements)
}

fn apply_replacements(lines: &[String], replacements: &[Replacement]) -> Vec<String> {
    let mut result = lines.to_vec();
    for (start_index, removed_len, inserted_lines) in replacements.iter().rev() {
        result.splice(
            *start_index..(*start_index + *removed_len),
            inserted_lines.clone(),
        );
    }
    result
}

fn seek_sequence(
    lines: &[String],
    pattern: &[String],
    start_index: usize,
    end_of_file: bool,
) -> Option<usize> {
    if pattern.is_empty() || pattern.len() > lines.len() {
        return None;
    }

    try_match(
        lines,
        pattern,
        start_index,
        end_of_file,
        |actual, expected| actual == expected,
    )
    .or_else(|| {
        try_match(
            lines,
            pattern,
            start_index,
            end_of_file,
            |actual, expected| actual.trim_end() == expected.trim_end(),
        )
    })
    .or_else(|| {
        try_match(
            lines,
            pattern,
            start_index,
            end_of_file,
            |actual, expected| actual.trim() == expected.trim(),
        )
    })
    .or_else(|| {
        try_match(
            lines,
            pattern,
            start_index,
            end_of_file,
            |actual, expected| {
                normalize_unicode(actual.trim()) == normalize_unicode(expected.trim())
            },
        )
    })
}

fn try_match<F>(
    lines: &[String],
    pattern: &[String],
    start_index: usize,
    end_of_file: bool,
    compare: F,
) -> Option<usize>
where
    F: Fn(&str, &str) -> bool,
{
    if pattern.len() > lines.len() {
        return None;
    }
    if end_of_file {
        let from_end = lines.len() - pattern.len();
        if from_end >= start_index
            && pattern
                .iter()
                .enumerate()
                .all(|(index, expected)| compare(&lines[from_end + index], expected))
        {
            return Some(from_end);
        }
    }

    for index in start_index..=(lines.len() - pattern.len()) {
        if pattern
            .iter()
            .enumerate()
            .all(|(offset, expected)| compare(&lines[index + offset], expected))
        {
            return Some(index);
        }
    }
    None
}

fn normalize_unicode(input: &str) -> String {
    input
        .replace(['\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'], "'")
        .replace(['\u{201C}', '\u{201D}', '\u{201E}', '\u{201F}'], "\"")
        .replace(
            [
                '\u{2010}', '\u{2011}', '\u{2012}', '\u{2013}', '\u{2014}', '\u{2015}',
            ],
            "-",
        )
        .replace('\u{2026}', "...")
        .replace('\u{00A0}', " ")
}

#[cfg(test)]
mod tests {
    use super::{derive_new_content, parse_patch_files, parse_patch_files_operations};
    use crate::ToolExecutionContext;
    use nanoclaw_test_support::run_current_thread_test;

    macro_rules! bounded_async_test {
        (async fn $name:ident() $body:block) => {
            #[test]
            fn $name() {
                run_current_thread_test(async $body);
            }
        };
    }

    #[test]
    fn parse_patch_reads_add_delete_and_update_hunks() {
        let hunks = parse_patch_files(
            "*** Begin Patch\n*** Add File: created.txt\n+hello\n*** Delete File: old.txt\n*** Update File: sample.txt\n@@\n-old\n+new\n*** End Patch",
        )
        .unwrap();

        assert_eq!(hunks.len(), 3);
    }

    #[test]
    fn derive_new_content_applies_fuzzy_chunk_matching() {
        let updated = derive_new_content(
            "sample.txt",
            "alpha  \n“beta”\n",
            &[super::UpdateChunk {
                change_context: Some("alpha".to_string()),
                old_lines: vec!["“beta”".to_string()],
                new_lines: vec!["beta".to_string()],
                is_end_of_file: true,
            }],
        )
        .unwrap();

        assert_eq!(updated, "alpha  \nbeta\n");
    }

    bounded_async_test!(
        async fn parse_patch_files_operations_reuses_patch_engine_for_updates() {
            let dir = tempfile::tempdir().unwrap();
            tokio::fs::write(dir.path().join("sample.txt"), "alpha\nbeta\n")
                .await
                .unwrap();

            let operations = parse_patch_files_operations(
                "*** Begin Patch\n*** Update File: sample.txt\n@@\n-beta\n+gamma\n*** End Patch",
                &ToolExecutionContext {
                    workspace_root: dir.path().to_path_buf(),
                    workspace_only: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

            assert_eq!(operations.len(), 1);
        }
    );

    bounded_async_test!(
        async fn parse_patch_files_operations_supports_move_only_hunks() {
            let dir = tempfile::tempdir().unwrap();
            tokio::fs::write(dir.path().join("sample.txt"), "alpha\nbeta\n")
                .await
                .unwrap();

            let operations = parse_patch_files_operations(
                "*** Begin Patch\n*** Update File: sample.txt\n*** Move to: renamed.txt\n*** End Patch",
                &ToolExecutionContext {
                    workspace_root: dir.path().to_path_buf(),
                    workspace_only: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

            assert_eq!(operations.len(), 1);
        }
    );

    bounded_async_test!(
        async fn parse_patch_files_operations_uses_end_of_file_anchor_for_repeated_lines() {
            let dir = tempfile::tempdir().unwrap();
            tokio::fs::write(dir.path().join("sample.txt"), "value\nkeep\nvalue\n")
                .await
                .unwrap();

            let operations = parse_patch_files_operations(
                "*** Begin Patch\n*** Update File: sample.txt\n@@\n-value\n+tail\n*** End of File\n*** End Patch",
                &ToolExecutionContext {
                    workspace_root: dir.path().to_path_buf(),
                    workspace_only: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

            assert_eq!(operations.len(), 1);
        }
    );

    #[test]
    fn parse_patch_rejects_empty_paths() {
        let error = parse_patch_files("*** Begin Patch\n*** Add File:   \n+hello\n*** End Patch")
            .unwrap_err();

        assert!(error.to_string().contains("path must not be empty"));
    }

    #[test]
    fn parse_patch_rejects_empty_change_lines_instead_of_panicking() {
        let error = parse_patch_files(
            "*** Begin Patch\n*** Update File: sample.txt\n@@\n-old\n\n+new\n*** End Patch",
        )
        .unwrap_err();

        assert!(error.to_string().contains("change lines must begin"));
    }
}
