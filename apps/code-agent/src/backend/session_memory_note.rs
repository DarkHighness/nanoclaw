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

pub(crate) fn render_session_memory_note(summary: &str) -> String {
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

    let mut lines = Vec::new();
    for section in SESSION_MEMORY_TEMPLATE {
        lines.push(format!("# {}", section.heading));
        lines.push(format!("_{}_", section.description));
        let body = section_body(&parsed.sections, section.heading);
        if !body.is_empty() {
            lines.push(String::new());
            lines.extend(body.lines().map(ToString::to_string));
        }
        lines.push(String::new());
    }

    lines.join("\n").trim_end().to_string()
}

pub(crate) fn default_session_memory_note() -> String {
    render_session_memory_note("")
}

pub(crate) fn strip_memory_frontmatter(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("---\n") else {
        return text;
    };
    let Some(frontmatter_end) = rest.find("\n---\n") else {
        return text;
    };
    &rest[frontmatter_end + "\n---\n".len()..]
}

pub(crate) fn build_session_memory_update_prompt(
    current_note: &str,
    transcript_delta: &str,
) -> String {
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
            "- Use only information grounded in the transcript delta.\n\n",
            "<current_session_note>\n",
            "{current_note}\n",
            "</current_session_note>\n\n",
            "<new_transcript_entries>\n",
            "{transcript_delta}\n",
            "</new_transcript_entries>\n"
        ),
        current_note = current_note.trim(),
        transcript_delta = transcript_delta.trim(),
    )
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
        build_session_memory_update_prompt, default_session_memory_note,
        render_session_memory_note, strip_memory_frontmatter,
    };

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
}
