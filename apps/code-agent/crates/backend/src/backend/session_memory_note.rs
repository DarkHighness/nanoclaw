use crate::backend::session_memory_compaction::session_memory_note_absolute_path;
use agent::memory::{
    MemoryBackend, MemoryMutationResponse, MemoryRecordMode, MemoryRecordRequest, MemoryScope,
    MemoryType,
};
use agent::types::{MessageId, SessionId};
use anyhow::Result;
use std::path::Path;
use tokio::fs;

const SESSION_MEMORY_SECTION_TOKEN_BUDGET: usize = 2_000;
const SESSION_MEMORY_TOTAL_TOKEN_BUDGET: usize = 12_000;
const SESSION_MEMORY_APPROX_CHARS_PER_TOKEN: usize = 4;
const SESSION_MEMORY_SECTION_TRUNCATION_MARKER: &str = "[... section truncated for length ...]";

struct SessionMemorySectionTemplate {
    heading: &'static str,
    description: &'static str,
}

const SESSION_MEMORY_TEMPLATE: &[SessionMemorySectionTemplate] = &[
    SessionMemorySectionTemplate {
        heading: "Session Title",
        description: "A short and distinctive 5-10 word descriptive title for the session. Super info dense, no filler",
    },
    SessionMemorySectionTemplate {
        heading: "Current State",
        description: "What is actively being worked on right now? Pending tasks not yet completed. Immediate next steps.",
    },
    SessionMemorySectionTemplate {
        heading: "Task specification",
        description: "What did the user ask to build? Any design decisions or other explanatory context",
    },
    SessionMemorySectionTemplate {
        heading: "Files and Functions",
        description: "What are the important files? In short, what do they contain and why are they relevant?",
    },
    SessionMemorySectionTemplate {
        heading: "Workflow",
        description: "What bash commands are usually run and in what order? How to interpret their output if not obvious?",
    },
    SessionMemorySectionTemplate {
        heading: "Errors & Corrections",
        description: "Errors encountered and how they were fixed. What did the user correct? What approaches failed and should not be tried again?",
    },
    SessionMemorySectionTemplate {
        heading: "Codebase and System Documentation",
        description: "What are the important system components? How do they work/fit together?",
    },
    SessionMemorySectionTemplate {
        heading: "Learnings",
        description: "What has worked well? What has not? What to avoid? Do not duplicate items from other sections",
    },
    SessionMemorySectionTemplate {
        heading: "Key results",
        description: "If the user asked a specific output such as an answer to a question, a table, or other document, repeat the exact result here",
    },
    SessionMemorySectionTemplate {
        heading: "Worklog",
        description: "Step by step, what was attempted, done? Very terse summary for each step",
    },
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ParsedSessionMemorySections {
    recognized_headings: usize,
    preface: String,
    sections: Vec<(&'static str, String)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionMemoryNoteSnapshot {
    pub body: String,
    pub last_summarized_message_id: Option<MessageId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionMemoryCompactContent {
    pub truncated_content: String,
    pub was_truncated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionMemoryNotePatch {
    pub session_title: Option<String>,
    pub current_state: Option<String>,
    pub task_specification: Option<String>,
    pub files_and_functions: Option<String>,
    pub workflow: Option<String>,
    pub errors_and_corrections: Option<String>,
    pub codebase_and_system_documentation: Option<String>,
    pub learnings: Option<String>,
    pub key_results: Option<String>,
    pub worklog: Option<String>,
}

impl SessionMemoryNotePatch {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.updated_sections().is_empty()
    }

    #[must_use]
    pub fn updated_sections(&self) -> Vec<&'static str> {
        let mut sections = Vec::new();
        if self.session_title.is_some() {
            sections.push("Session Title");
        }
        if self.current_state.is_some() {
            sections.push("Current State");
        }
        if self.task_specification.is_some() {
            sections.push("Task specification");
        }
        if self.files_and_functions.is_some() {
            sections.push("Files and Functions");
        }
        if self.workflow.is_some() {
            sections.push("Workflow");
        }
        if self.errors_and_corrections.is_some() {
            sections.push("Errors & Corrections");
        }
        if self.codebase_and_system_documentation.is_some() {
            sections.push("Codebase and System Documentation");
        }
        if self.learnings.is_some() {
            sections.push("Learnings");
        }
        if self.key_results.is_some() {
            sections.push("Key results");
        }
        if self.worklog.is_some() {
            sections.push("Worklog");
        }
        sections
    }
}

pub fn render_session_memory_note(summary: &str) -> String {
    let mut parsed = parse_session_memory_sections(summary);
    // The host owns the session-note shape so compaction output can stay
    // replaceable without changing the file skeleton that later resumes and
    // recalls depend on.
    if parsed.recognized_headings == 0 {
        set_section_body(
            &mut parsed.sections,
            "Current State",
            summary.trim().to_string(),
        );
    } else {
        let current_state = section_body(&parsed.sections, "Current State");
        if current_state.is_empty() {
            let fallback = if !parsed.preface.is_empty() {
                parsed.preface.clone()
            } else {
                parsed
                    .sections
                    .iter()
                    .find(|(heading, body)| *heading != "Session Title" && !body.trim().is_empty())
                    .map(|(_, body)| body.clone())
                    .unwrap_or_else(|| summary.trim().to_string())
            };
            set_section_body(&mut parsed.sections, "Current State", fallback);
        } else if !parsed.preface.is_empty() {
            set_section_body(
                &mut parsed.sections,
                "Current State",
                format!("{}\n\n{}", parsed.preface.trim(), current_state),
            );
        }
    }

    render_session_memory_sections(&parsed.sections)
}

pub fn default_session_memory_note() -> String {
    render_session_memory_note("")
}

pub fn strip_memory_frontmatter(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("---\n") else {
        return text;
    };
    let Some(frontmatter_end) = rest.find("\n---\n") else {
        return text;
    };
    &rest[frontmatter_end + "\n---\n".len()..]
}

pub fn parse_session_memory_note_snapshot(text: &str) -> SessionMemoryNoteSnapshot {
    SessionMemoryNoteSnapshot {
        body: strip_memory_frontmatter(text).trim().to_string(),
        last_summarized_message_id: extract_last_summarized_message_id(text),
    }
}

pub fn session_memory_note_title(text: &str) -> Option<String> {
    let snapshot = parse_session_memory_note_snapshot(text);
    let parsed = parse_session_memory_sections(&snapshot.body);
    let title = section_body(&parsed.sections, "Session Title");
    (!title.is_empty()).then_some(title)
}

pub fn upsert_session_memory_note_frontmatter(
    text: &str,
    last_summarized_message_id: Option<&MessageId>,
) -> String {
    let Some(rest) = text.strip_prefix("---\n") else {
        return text.to_string();
    };
    let Some(frontmatter_end) = rest.find("\n---\n") else {
        return text.to_string();
    };
    let frontmatter = &rest[..frontmatter_end];
    let body = &rest[frontmatter_end + "\n---\n".len()..];
    let mut lines = frontmatter
        .lines()
        .filter(|line| !line.trim_start().starts_with("last_summarized_message_id:"))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if let Some(last_summarized_message_id) = last_summarized_message_id {
        lines.push(format!(
            "last_summarized_message_id: {}",
            last_summarized_message_id.as_ref()
        ));
    }

    let mut rendered = String::from("---\n");
    if !lines.is_empty() {
        rendered.push_str(&lines.join("\n"));
        rendered.push('\n');
    }
    rendered.push_str("---\n");
    rendered.push_str(body);
    rendered
}

pub fn build_session_memory_update_prompt(current_note: &str, transcript_delta: &str) -> String {
    let budget_reminders = build_session_memory_budget_reminders(current_note);
    format!(
        concat!(
            "IMPORTANT: This request is internal session-note maintenance, not part of the user conversation.\n",
            "Return only the full updated session note in Markdown.\n\n",
            "CRITICAL RULES:\n",
            "- Preserve every section header exactly as written in the current note.\n",
            "- Preserve every italic description line exactly as written in the current note.\n",
            "- Only update the content that appears below each italic description line.\n",
            "- Do not add new sections, summaries, or meta commentary.\n",
            "- Do not mention note-taking instructions or this internal request.\n",
            "- Always refresh Current State.\n",
            "- Leave sections blank instead of adding filler.\n",
            "- Keep each section under roughly {section_budget} tokens by condensing older or lower-value details before the note sprawls.\n",
            "- Keep the full note under roughly {total_budget} tokens so it remains usable as post-compaction continuity.\n",
            "- Use only information grounded in the transcript delta.\n\n",
            "<current_session_note>\n",
            "{current_note}\n",
            "</current_session_note>\n\n",
            "<new_transcript_entries>\n",
            "{transcript_delta}\n",
            "</new_transcript_entries>\n",
            "{budget_reminders}"
        ),
        current_note = current_note.trim(),
        transcript_delta = transcript_delta.trim(),
        section_budget = SESSION_MEMORY_SECTION_TOKEN_BUDGET,
        total_budget = SESSION_MEMORY_TOTAL_TOKEN_BUDGET,
        budget_reminders = budget_reminders,
    )
}

#[must_use]
pub fn patch_session_memory_note(current_note: &str, patch: &SessionMemoryNotePatch) -> String {
    let note_body = strip_memory_frontmatter(current_note).trim();
    let base = if note_body.is_empty() {
        default_session_memory_note()
    } else {
        render_session_memory_note(note_body)
    };
    let mut parsed = parse_session_memory_sections(&base);

    apply_optional_section_patch(
        &mut parsed.sections,
        "Session Title",
        patch.session_title.as_deref(),
    );
    apply_optional_section_patch(
        &mut parsed.sections,
        "Current State",
        patch.current_state.as_deref(),
    );
    apply_optional_section_patch(
        &mut parsed.sections,
        "Task specification",
        patch.task_specification.as_deref(),
    );
    apply_optional_section_patch(
        &mut parsed.sections,
        "Files and Functions",
        patch.files_and_functions.as_deref(),
    );
    apply_optional_section_patch(&mut parsed.sections, "Workflow", patch.workflow.as_deref());
    apply_optional_section_patch(
        &mut parsed.sections,
        "Errors & Corrections",
        patch.errors_and_corrections.as_deref(),
    );
    apply_optional_section_patch(
        &mut parsed.sections,
        "Codebase and System Documentation",
        patch.codebase_and_system_documentation.as_deref(),
    );
    apply_optional_section_patch(
        &mut parsed.sections,
        "Learnings",
        patch.learnings.as_deref(),
    );
    apply_optional_section_patch(
        &mut parsed.sections,
        "Key results",
        patch.key_results.as_deref(),
    );
    apply_optional_section_patch(&mut parsed.sections, "Worklog", patch.worklog.as_deref());

    render_session_memory_sections(&parsed.sections)
}

pub async fn load_session_memory_note_snapshot(
    workspace_root: &Path,
    session_id: &SessionId,
) -> Result<Option<SessionMemoryNoteSnapshot>> {
    let path = session_memory_note_absolute_path(workspace_root, session_id);
    match fs::read_to_string(path).await {
        Ok(text) => Ok(Some(parse_session_memory_note_snapshot(&text))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub async fn persist_session_memory_note(
    workspace_root: &Path,
    memory_backend: &dyn MemoryBackend,
    session_id: &SessionId,
    agent_session_id: &agent::types::AgentSessionId,
    note: String,
    last_summarized_message_id: Option<&MessageId>,
    tags: Vec<String>,
) -> Result<MemoryMutationResponse> {
    let response = memory_backend
        .record(MemoryRecordRequest {
            scope: MemoryScope::Working,
            title: "Session continuation snapshot".to_string(),
            content: note,
            mode: MemoryRecordMode::Replace,
            memory_type: Some(MemoryType::Project),
            description: Some(
                "Latest structured session note for the current runtime session.".to_string(),
            ),
            layer: Some("session".to_string()),
            tags,
            session_id: Some(session_id.clone()),
            agent_session_id: Some(agent_session_id.clone()),
            agent_name: None,
            task_id: None,
        })
        .await?;
    // The generic memory backend owns note file writes, but the session
    // continuity boundary is host-specific. Patch the same file's frontmatter
    // immediately after the managed write so resume and future compaction
    // decisions read one durable source of truth.
    let path = session_memory_note_absolute_path(workspace_root, session_id);
    let text = fs::read_to_string(&path).await?;
    let patched = upsert_session_memory_note_frontmatter(&text, last_summarized_message_id);
    if patched != text {
        fs::write(path, patched).await?;
    }
    Ok(response)
}

pub fn truncate_session_memory_for_compaction(content: &str) -> SessionMemoryCompactContent {
    let max_chars_per_section =
        SESSION_MEMORY_SECTION_TOKEN_BUDGET * SESSION_MEMORY_APPROX_CHARS_PER_TOKEN;
    let mut output_lines = Vec::new();
    let mut current_section_header = String::new();
    let mut current_section_lines = Vec::new();
    let mut was_truncated = false;

    for line in content.lines() {
        if line.starts_with("# ") {
            let result = flush_session_memory_section(
                &current_section_header,
                &current_section_lines,
                max_chars_per_section,
            );
            output_lines.extend(result.lines);
            was_truncated |= result.was_truncated;
            current_section_header = line.to_string();
            current_section_lines.clear();
        } else {
            current_section_lines.push(line.to_string());
        }
    }

    let result = flush_session_memory_section(
        &current_section_header,
        &current_section_lines,
        max_chars_per_section,
    );
    output_lines.extend(result.lines);
    was_truncated |= result.was_truncated;

    SessionMemoryCompactContent {
        truncated_content: output_lines.join("\n"),
        was_truncated,
    }
}

fn extract_last_summarized_message_id(text: &str) -> Option<MessageId> {
    let rest = text.strip_prefix("---\n")?;
    let frontmatter_end = rest.find("\n---\n")?;
    rest[..frontmatter_end].lines().find_map(|line| {
        line.trim_start()
            .strip_prefix("last_summarized_message_id:")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(MessageId::from)
    })
}

fn parse_session_memory_sections(summary: &str) -> ParsedSessionMemorySections {
    let mut parsed = ParsedSessionMemorySections {
        sections: SESSION_MEMORY_TEMPLATE
            .iter()
            .map(|section| (section.heading, String::new()))
            .collect(),
        ..Default::default()
    };
    let mut active_heading = None;
    let mut buffer = Vec::new();
    let mut preface = Vec::new();

    for line in summary.lines() {
        let Some(heading) = parse_template_heading(line) else {
            if active_heading.is_some() {
                buffer.push(line);
            } else {
                preface.push(line);
            }
            continue;
        };

        if let Some(previous_heading) = active_heading.take() {
            append_section_body(
                &mut parsed.sections,
                previous_heading,
                sanitize_section_body(previous_heading, &buffer.join("\n")),
            );
            buffer.clear();
        }
        parsed.recognized_headings += 1;
        active_heading = Some(heading);
    }

    if let Some(previous_heading) = active_heading {
        append_section_body(
            &mut parsed.sections,
            previous_heading,
            sanitize_section_body(previous_heading, &buffer.join("\n")),
        );
    }

    parsed.preface = preface.join("\n").trim().to_string();
    parsed
}

fn build_session_memory_budget_reminders(current_note: &str) -> String {
    let section_sizes = analyze_session_memory_section_sizes(current_note);
    let total_tokens = estimate_session_memory_tokens(current_note);
    let oversized_sections = section_sizes
        .into_iter()
        .filter(|(_, tokens)| *tokens > SESSION_MEMORY_SECTION_TOKEN_BUDGET)
        .collect::<Vec<_>>();
    if oversized_sections.is_empty() && total_tokens <= SESSION_MEMORY_TOTAL_TOKEN_BUDGET {
        return String::new();
    }

    let mut parts = Vec::new();
    if total_tokens > SESSION_MEMORY_TOTAL_TOKEN_BUDGET {
        parts.push(format!(
            "\n\nCRITICAL: The session note is currently ~{total_tokens} tokens, which exceeds the target budget of {total_budget} tokens. Condense it while preserving the most important continuity details. Prioritize keeping `Current State` and `Errors & Corrections` accurate.\n",
            total_budget = SESSION_MEMORY_TOTAL_TOKEN_BUDGET,
        ));
    }
    if !oversized_sections.is_empty() {
        let oversized_lines = oversized_sections
            .into_iter()
            .map(|(section, tokens)| {
                format!(
                    "- \"{section}\" is ~{tokens} tokens (limit: {limit})",
                    limit = SESSION_MEMORY_SECTION_TOKEN_BUDGET,
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        parts.push(format!(
            "\n\nIMPORTANT: Condense these oversized sections before adding more detail:\n{oversized_lines}\n"
        ));
    }

    parts.join("")
}

fn analyze_session_memory_section_sizes(content: &str) -> Vec<(String, usize)> {
    let mut sections = Vec::new();
    let mut current_section = None;
    let mut current_lines = Vec::new();

    for line in content.lines() {
        if line.starts_with("# ") {
            if let Some(current_section) = current_section.take() {
                sections.push((
                    current_section,
                    estimate_session_memory_tokens(&current_lines.join("\n")),
                ));
                current_lines.clear();
            }
            current_section = Some(line.trim().to_string());
        } else {
            current_lines.push(line.to_string());
        }
    }

    if let Some(current_section) = current_section {
        sections.push((
            current_section,
            estimate_session_memory_tokens(&current_lines.join("\n")),
        ));
    }
    sections
}

fn estimate_session_memory_tokens(text: &str) -> usize {
    text.len().div_ceil(SESSION_MEMORY_APPROX_CHARS_PER_TOKEN)
}

fn flush_session_memory_section(
    section_header: &str,
    section_lines: &[String],
    max_chars_per_section: usize,
) -> FlushedSessionMemorySection {
    if section_header.is_empty() {
        return FlushedSessionMemorySection {
            lines: section_lines.to_vec(),
            was_truncated: false,
        };
    }

    let section_content = section_lines.join("\n");
    if section_content.len() <= max_chars_per_section {
        let mut lines = vec![section_header.to_string()];
        lines.extend(section_lines.to_vec());
        return FlushedSessionMemorySection {
            lines,
            was_truncated: false,
        };
    }

    let mut char_count = 0;
    let mut kept_lines = vec![section_header.to_string()];
    for line in section_lines {
        if char_count + line.len() + 1 > max_chars_per_section {
            break;
        }
        kept_lines.push(line.clone());
        char_count += line.len() + 1;
    }
    kept_lines.push(String::new());
    kept_lines.push(SESSION_MEMORY_SECTION_TRUNCATION_MARKER.to_string());
    FlushedSessionMemorySection {
        lines: kept_lines,
        was_truncated: true,
    }
}

struct FlushedSessionMemorySection {
    lines: Vec<String>,
    was_truncated: bool,
}

fn parse_template_heading(line: &str) -> Option<&'static str> {
    let heading = line
        .trim_start()
        .strip_prefix('#')?
        .trim_start_matches('#')
        .trim();
    SESSION_MEMORY_TEMPLATE
        .iter()
        .find(|section| section.heading.eq_ignore_ascii_case(heading))
        .map(|section| section.heading)
}

fn render_session_memory_sections(sections: &[(&'static str, String)]) -> String {
    let mut lines = Vec::new();
    for section in SESSION_MEMORY_TEMPLATE {
        lines.push(format!("# {}", section.heading));
        lines.push(format!("_{}_", section.description));
        let body = section_body(sections, section.heading);
        if !body.is_empty() {
            lines.push(String::new());
            lines.extend(body.lines().map(ToString::to_string));
        }
        lines.push(String::new());
    }

    lines.join("\n").trim_end().to_string()
}

fn sanitize_section_body(heading: &str, body: &str) -> String {
    let Some(template) = SESSION_MEMORY_TEMPLATE
        .iter()
        .find(|section| section.heading == heading)
    else {
        return body.trim().to_string();
    };

    body.lines()
        .filter(|line| {
            let trimmed = line.trim();
            trimmed != format!("_{}_", template.description)
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn append_section_body(
    sections: &mut [(&'static str, String)],
    heading: &'static str,
    body: String,
) {
    if body.is_empty() {
        return;
    }
    if let Some((_, existing)) = sections.iter_mut().find(|(name, _)| *name == heading) {
        if !existing.is_empty() {
            existing.push_str("\n\n");
        }
        existing.push_str(&body);
    }
}

fn set_section_body(sections: &mut [(&'static str, String)], heading: &'static str, body: String) {
    if let Some((_, existing)) = sections.iter_mut().find(|(name, _)| *name == heading) {
        *existing = body.trim().to_string();
    }
}

fn apply_optional_section_patch(
    sections: &mut [(&'static str, String)],
    heading: &'static str,
    body: Option<&str>,
) {
    if let Some(body) = body {
        set_section_body(sections, heading, body.to_string());
    }
}

fn section_body(sections: &[(&'static str, String)], heading: &'static str) -> String {
    sections
        .iter()
        .find(|(name, _)| *name == heading)
        .map(|(_, body)| body.trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        SESSION_MEMORY_APPROX_CHARS_PER_TOKEN, SESSION_MEMORY_SECTION_TOKEN_BUDGET,
        SESSION_MEMORY_TOTAL_TOKEN_BUDGET, SessionMemoryNotePatch,
        build_session_memory_update_prompt, default_session_memory_note,
        parse_session_memory_note_snapshot, patch_session_memory_note, render_session_memory_note,
        session_memory_note_title, strip_memory_frontmatter,
        truncate_session_memory_for_compaction, upsert_session_memory_note_frontmatter,
    };
    use agent::types::MessageId;

    #[test]
    fn freeform_summary_falls_back_to_current_state_in_stable_template() {
        let rendered =
            render_session_memory_note("Need to finish deploy rollback and rerun smoke tests.");

        assert!(rendered.contains("# Session Title"));
        assert!(rendered.contains("# Current State"));
        assert!(rendered.contains("# Worklog"));
        assert!(rendered.contains("Need to finish deploy rollback and rerun smoke tests."));
    }

    #[test]
    fn recognized_sections_fill_matching_template_slots() {
        let rendered = render_session_memory_note(
            "# Session Title\n\nDeploy rollback follow-up\n\n# Files and Functions\n\n- services/api.rs\n- runtime/guard.rs\n\n# Worklog\n\n- reran rollback\n- captured logs",
        );

        assert!(rendered.contains("# Session Title\n_A short and distinctive"));
        assert!(rendered.contains("Deploy rollback follow-up"));
        assert!(rendered.contains("# Files and Functions"));
        assert!(rendered.contains("- services/api.rs"));
        assert!(rendered.contains("# Worklog"));
        assert!(rendered.contains("- reran rollback"));
    }

    #[test]
    fn missing_current_state_reuses_first_non_title_section_for_continuity() {
        let rendered = render_session_memory_note(
            "# Task specification\n\nShip the compaction renderer.\n\n# Worklog\n\n- added parser",
        );

        let current_state_block = rendered
            .split("# Current State")
            .nth(1)
            .expect("current state block");
        assert!(current_state_block.contains("Ship the compaction renderer."));
    }

    #[test]
    fn preface_text_is_preserved_inside_current_state() {
        let rendered = render_session_memory_note(
            "Keep deploy paused until smoke tests pass.\n\n# Current State\n\nFix the session note renderer.",
        );

        let current_state_block = rendered
            .split("# Current State")
            .nth(1)
            .expect("current state block");
        assert!(current_state_block.contains("Keep deploy paused until smoke tests pass."));
        assert!(current_state_block.contains("Fix the session note renderer."));
    }

    #[test]
    fn default_note_keeps_the_full_template_shape() {
        let note = default_session_memory_note();

        assert!(note.contains("# Session Title"));
        assert!(note.contains("# Current State"));
        assert!(note.contains("# Worklog"));
    }

    #[test]
    fn strip_memory_frontmatter_returns_markdown_body() {
        let stripped =
            strip_memory_frontmatter("---\nscope: working\n---\n\n# Current State\n\nKeep going.");

        assert_eq!(stripped.trim(), "# Current State\n\nKeep going.");
    }

    #[test]
    fn update_prompt_embeds_current_note_and_transcript_delta() {
        let prompt = build_session_memory_update_prompt(
            "# Current State\n\nFix it.",
            "user> what changed?\n\nassistant> refreshed note",
        );

        assert!(prompt.contains("<current_session_note>"));
        assert!(prompt.contains("<new_transcript_entries>"));
        assert!(prompt.contains("Always refresh Current State"));
    }

    #[test]
    fn update_prompt_warns_about_section_and_total_budget_pressure() {
        let oversized = "x".repeat(
            (SESSION_MEMORY_TOTAL_TOKEN_BUDGET + 200) * SESSION_MEMORY_APPROX_CHARS_PER_TOKEN,
        );
        let prompt = build_session_memory_update_prompt(
            format!("# Current State\n\n{oversized}").as_str(),
            "assistant> keep only the critical details",
        );

        assert!(prompt.contains("exceeds the target budget"));
        assert!(prompt.contains("\"# Current State\" is ~"));
        assert!(prompt.contains("Condense these oversized sections"));
    }

    #[test]
    fn compaction_truncation_preserves_structure_and_marks_oversized_sections() {
        let oversized = "line\n".repeat(SESSION_MEMORY_SECTION_TOKEN_BUDGET * 4);
        let content =
            format!("# Current State\n_state_\n\n{oversized}\n# Worklog\n_steps_\n\n- kept");

        let truncated = truncate_session_memory_for_compaction(&content);

        assert!(truncated.was_truncated);
        assert!(truncated.truncated_content.contains("# Current State"));
        assert!(truncated.truncated_content.contains("_state_"));
        assert!(
            truncated
                .truncated_content
                .contains("[... section truncated for length ...]")
        );
        assert!(truncated.truncated_content.contains("# Worklog"));
        assert!(truncated.truncated_content.contains("- kept"));
    }

    #[test]
    fn compaction_truncation_leaves_small_notes_unchanged() {
        let content = "# Current State\n_state_\n\nKeep going.\n";

        let truncated = truncate_session_memory_for_compaction(content);

        assert!(!truncated.was_truncated);
        assert_eq!(truncated.truncated_content, content.trim_end());
    }

    #[test]
    fn snapshot_parser_reads_last_summarized_message_id_from_frontmatter() {
        let snapshot = parse_session_memory_note_snapshot(
            "---\nscope: working\nlast_summarized_message_id: msg_123\n---\n\n# Current State\n\nKeep going.",
        );

        assert_eq!(
            snapshot.last_summarized_message_id,
            Some(MessageId::from("msg_123"))
        );
        assert_eq!(snapshot.body, "# Current State\n\nKeep going.");
    }

    #[test]
    fn upsert_session_memory_note_frontmatter_replaces_boundary_line() {
        let text = upsert_session_memory_note_frontmatter(
            "---\nscope: working\nlast_summarized_message_id: old\n---\n\n# Current State\n\nKeep going.\n",
            Some(&MessageId::from("msg_456")),
        );

        assert!(text.contains("last_summarized_message_id: msg_456"));
        assert!(!text.contains("last_summarized_message_id: old"));
    }

    #[test]
    fn session_memory_note_title_reads_structured_title_section() {
        let note = concat!(
            "---\n",
            "scope: working\n",
            "---\n\n",
            "# Session Title\n",
            "_A short and distinctive 5-10 word descriptive title for the session. Super info dense, no filler_\n\n",
            "Deploy rollback follow-up\n\n",
            "# Current State\n",
            "_What is actively being worked on right now? Pending tasks not yet completed. Immediate next steps._\n\n",
            "Validate the hotfix.\n"
        );

        assert_eq!(
            session_memory_note_title(note).as_deref(),
            Some("Deploy rollback follow-up")
        );
    }

    #[test]
    fn session_memory_note_title_ignores_blank_title_section() {
        let note = default_session_memory_note();

        assert_eq!(session_memory_note_title(&note), None);
    }

    #[test]
    fn patch_session_memory_note_preserves_omitted_sections() {
        let note = render_session_memory_note(
            "# Current State\n\nOld state.\n\n# Files and Functions\n\n- src/lib.rs",
        );
        let patched = patch_session_memory_note(
            &note,
            &SessionMemoryNotePatch {
                current_state: Some("New state.".to_string()),
                ..SessionMemoryNotePatch::default()
            },
        );

        assert!(patched.contains("New state."));
        assert!(patched.contains("- src/lib.rs"));
        assert!(!patched.contains("Old state."));
    }
}
