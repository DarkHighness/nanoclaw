use agent::AgentWorkspaceLayout;
use agent::types::SubmittedPromptSnapshot;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use tracing::warn;

const COMPOSER_HISTORY_FILE_NAME: &str = "code-agent-prompt-history.jsonl";
pub(crate) const MAX_COMPOSER_HISTORY_ENTRIES: usize = 200;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ComposerHistoryKind {
    #[default]
    Prompt,
    Command,
}

impl ComposerHistoryKind {
    pub(crate) fn classify_text(text: &str) -> Self {
        if text.trim_start().starts_with('/') {
            Self::Command
        } else {
            Self::Prompt
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PersistedComposerHistoryEntry {
    #[serde(default)]
    pub(crate) kind: ComposerHistoryKind,
    pub(crate) prompt: SubmittedPromptSnapshot,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LoadedComposerHistory {
    pub(crate) entries: Vec<PersistedComposerHistoryEntry>,
    pub(crate) prompts: Vec<SubmittedPromptSnapshot>,
    pub(crate) commands: Vec<SubmittedPromptSnapshot>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum StoredComposerHistoryLine {
    Typed(PersistedComposerHistoryEntry),
    Legacy(SubmittedPromptSnapshot),
}

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
    kind: ComposerHistoryKind,
    mut prompt: SubmittedPromptSnapshot,
) -> Option<PersistedComposerHistoryEntry> {
    if let Some(text) = normalized_history_text(&prompt.text) {
        prompt.text = text;
        return Some(PersistedComposerHistoryEntry { kind, prompt });
    }
    if prompt.attachments.is_empty() {
        return None;
    }
    prompt.text.clear();
    Some(PersistedComposerHistoryEntry { kind, prompt })
}

pub(crate) fn record_input_history(
    entries: &mut Vec<PersistedComposerHistoryEntry>,
    kind: ComposerHistoryKind,
    prompt: SubmittedPromptSnapshot,
) -> bool {
    let Some(entry) = normalized_history_entry(kind, prompt) else {
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

pub(crate) fn load_input_history(workspace_root: &Path) -> LoadedComposerHistory {
    let path = composer_history_path(workspace_root);
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return LoadedComposerHistory::default();
        }
        Err(error) => {
            warn!(
                path = %path.display(),
                error = %error,
                "failed to open composer history file"
            );
            return LoadedComposerHistory::default();
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
        let parsed = match serde_json::from_str::<StoredComposerHistoryLine>(&line) {
            Ok(StoredComposerHistoryLine::Typed(entry)) => entry,
            Ok(StoredComposerHistoryLine::Legacy(prompt)) => PersistedComposerHistoryEntry {
                kind: ComposerHistoryKind::classify_text(&prompt.text),
                prompt,
            },
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
        let _ = record_input_history(&mut entries, parsed.kind, parsed.prompt);
    }

    let mut loaded = LoadedComposerHistory::default();
    for entry in entries {
        match entry.kind {
            ComposerHistoryKind::Prompt => loaded.prompts.push(entry.prompt.clone()),
            ComposerHistoryKind::Command => loaded.commands.push(entry.prompt.clone()),
        }
        loaded.entries.push(entry);
    }
    loaded
}

pub(crate) fn persist_input_history(
    workspace_root: &Path,
    entries: &[PersistedComposerHistoryEntry],
) {
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
        if let Err(error) = serde_json::to_writer(
            &mut writer,
            &StoredComposerHistoryLine::Typed(entry.clone()),
        ) {
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
        ComposerHistoryKind, MAX_COMPOSER_HISTORY_ENTRIES, PersistedComposerHistoryEntry,
        load_input_history, persist_input_history, record_input_history,
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
            ComposerHistoryKind::Prompt,
            SubmittedPromptSnapshot::from_text("   ")
        ));
        assert!(record_input_history(
            &mut entries,
            ComposerHistoryKind::Prompt,
            SubmittedPromptSnapshot::from_text(" first ")
        ));
        assert!(!record_input_history(
            &mut entries,
            ComposerHistoryKind::Prompt,
            SubmittedPromptSnapshot::from_text("first")
        ));
        assert!(record_input_history(
            &mut entries,
            ComposerHistoryKind::Prompt,
            SubmittedPromptSnapshot::from_text("second")
        ));

        assert_eq!(
            entries,
            vec![
                super::PersistedComposerHistoryEntry {
                    kind: ComposerHistoryKind::Prompt,
                    prompt: SubmittedPromptSnapshot::from_text("first"),
                },
                super::PersistedComposerHistoryEntry {
                    kind: ComposerHistoryKind::Prompt,
                    prompt: SubmittedPromptSnapshot::from_text("second"),
                }
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
                PersistedComposerHistoryEntry {
                    kind: ComposerHistoryKind::Prompt,
                    prompt: SubmittedPromptSnapshot::from_text("prompt one"),
                },
                PersistedComposerHistoryEntry {
                    kind: ComposerHistoryKind::Command,
                    prompt: SubmittedPromptSnapshot::from_text("/help"),
                },
                PersistedComposerHistoryEntry {
                    kind: ComposerHistoryKind::Prompt,
                    prompt: SubmittedPromptSnapshot::from_text("prompt two"),
                },
            ],
        );
        persist_input_history(
            second.path(),
            &[PersistedComposerHistoryEntry {
                kind: ComposerHistoryKind::Prompt,
                prompt: SubmittedPromptSnapshot::from_text("other workspace"),
            }],
        );

        let first_loaded = load_input_history(first.path());
        assert_eq!(
            first_loaded.prompts,
            vec![
                SubmittedPromptSnapshot::from_text("prompt one"),
                SubmittedPromptSnapshot::from_text("prompt two")
            ]
        );
        assert_eq!(
            first_loaded.commands,
            vec![SubmittedPromptSnapshot::from_text("/help")]
        );
        assert_eq!(
            first_loaded
                .entries
                .into_iter()
                .map(|entry| (entry.kind, entry.prompt.text))
                .collect::<Vec<_>>(),
            vec![
                (ComposerHistoryKind::Prompt, "prompt one".to_string()),
                (ComposerHistoryKind::Command, "/help".to_string()),
                (ComposerHistoryKind::Prompt, "prompt two".to_string()),
            ]
        );

        let second_loaded = load_input_history(second.path());
        assert_eq!(
            second_loaded.prompts,
            vec![SubmittedPromptSnapshot::from_text("other workspace")]
        );
        assert!(second_loaded.commands.is_empty());
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

        let loaded = load_input_history(dir.path());
        assert_eq!(
            loaded.prompts,
            vec![SubmittedPromptSnapshot::from_text("prompt one")]
        );
        assert!(loaded.commands.is_empty());
    }

    #[test]
    fn persisted_history_classifies_legacy_slash_commands_into_command_history() {
        let dir = tempdir().unwrap();
        let apps_dir = AgentWorkspaceLayout::new(dir.path()).apps_dir();
        std::fs::create_dir_all(&apps_dir).unwrap();
        std::fs::write(
            apps_dir.join("code-agent-prompt-history.jsonl"),
            "{\"text\":\"/help\"}\n",
        )
        .unwrap();

        let loaded = load_input_history(dir.path());
        assert!(loaded.prompts.is_empty());
        assert_eq!(
            loaded.commands,
            vec![SubmittedPromptSnapshot::from_text("/help")]
        );
    }

    #[test]
    fn attachment_only_entries_are_kept() {
        let mut entries = Vec::new();

        assert!(record_input_history(
            &mut entries,
            ComposerHistoryKind::Prompt,
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
                ComposerHistoryKind::Prompt,
                SubmittedPromptSnapshot::from_text(format!("prompt {index}"))
            ));
        }

        let expected_last = format!("prompt {}", MAX_COMPOSER_HISTORY_ENTRIES + 4);
        assert_eq!(entries.len(), MAX_COMPOSER_HISTORY_ENTRIES);
        assert_eq!(
            entries.first().map(|entry| entry.prompt.text.as_str()),
            Some("prompt 5")
        );
        assert_eq!(
            entries.last().map(|entry| entry.prompt.text.as_str()),
            Some(expected_last.as_str())
        );
    }
}
