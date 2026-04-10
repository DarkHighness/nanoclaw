use agent::AgentWorkspaceLayout;
use agent::types::SubmittedPromptSnapshot;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use tracing::warn;

const COMPOSER_HISTORY_FILE_NAME: &str = "code-agent-prompt-history.jsonl";
pub(crate) const MAX_COMPOSER_HISTORY_ENTRIES: usize = 200;

fn composer_history_path(workspace_root: &Path) -> PathBuf {
    AgentWorkspaceLayout::new(workspace_root)
        .apps_dir()
        .join(COMPOSER_HISTORY_FILE_NAME)
}

pub(crate) fn normalized_history_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub(crate) fn normalized_history_entry(
    mut entry: SubmittedPromptSnapshot,
) -> Option<SubmittedPromptSnapshot> {
    if let Some(text) = normalized_history_text(&entry.text) {
        entry.text = text;
        return Some(entry);
    }
    if entry.attachments.is_empty() {
        return None;
    }
    entry.text.clear();
    Some(entry)
}

pub(crate) fn record_input_history(
    entries: &mut Vec<SubmittedPromptSnapshot>,
    entry: SubmittedPromptSnapshot,
) -> bool {
    let Some(entry) = normalized_history_entry(entry) else {
        return false;
    };
    if entries.last() == Some(&entry) {
        return false;
    }
    entries.push(entry);
    if entries.len() > MAX_COMPOSER_HISTORY_ENTRIES {
        let overflow = entries.len() - MAX_COMPOSER_HISTORY_ENTRIES;
        entries.drain(0..overflow);
    }
    true
}

pub(crate) fn load_input_history(workspace_root: &Path) -> Vec<SubmittedPromptSnapshot> {
    let path = composer_history_path(workspace_root);
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            warn!(
                path = %path.display(),
                error = %error,
                "failed to open composer history file"
            );
            return Vec::new();
        }
    };

    let mut entries = Vec::new();
    for (line_number, line) in BufReader::new(file).lines().enumerate() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                warn!(
                    path = %path.display(),
                    line = line_number + 1,
                    error = %error,
                    "failed to read composer history line"
                );
                continue;
            }
        };
        let parsed = match serde_json::from_str::<SubmittedPromptSnapshot>(&line) {
            Ok(entry) => entry,
            Err(error) => {
                warn!(
                    path = %path.display(),
                    line = line_number + 1,
                    error = %error,
                    "failed to parse composer history line"
                );
                continue;
            }
        };
        let _ = record_input_history(&mut entries, parsed);
    }

    entries
}

pub(crate) fn persist_input_history(workspace_root: &Path, entries: &[SubmittedPromptSnapshot]) {
    let path = composer_history_path(workspace_root);
    if let Some(parent) = path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        warn!(
            path = %parent.display(),
            error = %error,
            "failed to create composer history directory"
        );
        return;
    }

    let file = match File::create(&path) {
        Ok(file) => file,
        Err(error) => {
            warn!(
                path = %path.display(),
                error = %error,
                "failed to create composer history file"
            );
            return;
        }
    };
    let mut writer = BufWriter::new(file);
    for entry in entries {
        if let Err(error) = serde_json::to_writer(&mut writer, entry) {
            warn!(
                path = %path.display(),
                error = %error,
                "failed to serialize composer history entry"
            );
            return;
        }
        if let Err(error) = writer.write_all(b"\n") {
            warn!(
                path = %path.display(),
                error = %error,
                "failed to write composer history newline"
            );
            return;
        }
    }
    if let Err(error) = writer.flush() {
        warn!(
            path = %path.display(),
            error = %error,
            "failed to flush composer history file"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_COMPOSER_HISTORY_ENTRIES, load_input_history, persist_input_history,
        record_input_history,
    };
    use agent::AgentWorkspaceLayout;
    use agent::types::{
        SubmittedPromptAttachment, SubmittedPromptAttachmentKind, SubmittedPromptSnapshot,
    };
    use tempfile::tempdir;

    #[test]
    fn record_input_history_trims_and_deduplicates_tail_entries() {
        let mut entries = Vec::new();

        assert!(!record_input_history(
            &mut entries,
            SubmittedPromptSnapshot::from_text("   ")
        ));
        assert!(record_input_history(
            &mut entries,
            SubmittedPromptSnapshot::from_text(" first ")
        ));
        assert!(!record_input_history(
            &mut entries,
            SubmittedPromptSnapshot::from_text("first")
        ));
        assert!(record_input_history(
            &mut entries,
            SubmittedPromptSnapshot::from_text("second")
        ));

        assert_eq!(
            entries,
            vec![
                SubmittedPromptSnapshot::from_text("first"),
                SubmittedPromptSnapshot::from_text("second")
            ]
        );
    }

    #[test]
    fn persisted_history_round_trips_per_workspace() {
        let first = tempdir().unwrap();
        let second = tempdir().unwrap();

        persist_input_history(
            first.path(),
            &[
                SubmittedPromptSnapshot::from_text("prompt one"),
                SubmittedPromptSnapshot::from_text("prompt two"),
            ],
        );
        persist_input_history(
            second.path(),
            &[SubmittedPromptSnapshot::from_text("other workspace")],
        );

        assert_eq!(
            load_input_history(first.path()),
            vec![
                SubmittedPromptSnapshot::from_text("prompt one"),
                SubmittedPromptSnapshot::from_text("prompt two")
            ]
        );
        assert_eq!(
            load_input_history(second.path()),
            vec![SubmittedPromptSnapshot::from_text("other workspace")]
        );
    }

    #[test]
    fn persisted_history_reads_legacy_text_only_entries() {
        let dir = tempdir().unwrap();
        let apps_dir = AgentWorkspaceLayout::new(dir.path()).apps_dir();
        std::fs::create_dir_all(&apps_dir).unwrap();
        std::fs::write(
            apps_dir.join("code-agent-prompt-history.jsonl"),
            "{\"text\":\"prompt one\"}\n",
        )
        .unwrap();

        assert_eq!(
            load_input_history(dir.path()),
            vec![SubmittedPromptSnapshot::from_text("prompt one")]
        );
    }

    #[test]
    fn attachment_only_entries_are_kept() {
        let mut entries = Vec::new();

        assert!(record_input_history(
            &mut entries,
            SubmittedPromptSnapshot {
                text: String::new(),
                attachments: vec![SubmittedPromptAttachment {
                    placeholder: Some("[Image #1]".to_string()),
                    kind: SubmittedPromptAttachmentKind::LocalImage {
                        requested_path: "artifacts/failure.png".to_string(),
                        mime_type: Some("image/png".to_string()),
                    },
                }],
            }
        ));

        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn persisted_history_is_bounded() {
        let mut entries = Vec::new();
        for index in 0..(MAX_COMPOSER_HISTORY_ENTRIES + 5) {
            assert!(record_input_history(
                &mut entries,
                SubmittedPromptSnapshot::from_text(format!("prompt {index}"))
            ));
        }

        let expected_last = format!("prompt {}", MAX_COMPOSER_HISTORY_ENTRIES + 4);
        assert_eq!(entries.len(), MAX_COMPOSER_HISTORY_ENTRIES);
        assert_eq!(
            entries.first().map(|entry| entry.text.as_str()),
            Some("prompt 5")
        );
        assert_eq!(
            entries.last().map(|entry| entry.text.as_str()),
            Some(expected_last.as_str())
        );
    }
}
