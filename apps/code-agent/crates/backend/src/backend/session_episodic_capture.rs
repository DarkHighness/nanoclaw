pub fn build_session_episodic_capture_prompt(transcript_delta: &str) -> String {
    format!(
        concat!(
            "You are maintaining an append-only episodic daily log for this workspace.\n\n",
            "Review ONLY the transcript delta below and extract at most 5 concise bullets that are worth preserving for later memory consolidation or future operator recall.\n\n",
            "Prefer items like:\n",
            "- user preferences, corrections, or collaboration constraints\n",
            "- non-derivable project context, incidents, decisions, and rationale\n",
            "- pointers to external systems, dashboards, or coordination threads\n",
            "- anything the user explicitly asked to remember\n\n",
            "Do NOT log:\n",
            "- routine implementation steps already obvious from the repository state\n",
            "- speculative claims or unverifiable inferences\n",
            "- transient TODOs or plans that only matter for the current turn\n\n",
            "Return ONLY Markdown bullet lines starting with `- `. If nothing is worth logging, return `NONE`.\n\n",
            "## Transcript Delta\n\n",
            "{transcript_delta}"
        ),
        transcript_delta = transcript_delta.trim()
    )
}

pub fn parse_session_episodic_capture_entries(text: &str) -> Vec<String> {
    let stripped = strip_optional_code_fence(text);
    let stripped = stripped.trim();
    if stripped.is_empty() || stripped.eq_ignore_ascii_case("none") {
        return Vec::new();
    }

    let mut entries = Vec::new();
    let mut current = Vec::new();
    for line in stripped.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(entry) = strip_bullet_marker(trimmed) {
            if !current.is_empty() {
                entries.push(current.join("\n"));
                current.clear();
            }
            current.push(entry.to_string());
            continue;
        }
        current.push(trimmed.to_string());
    }
    if !current.is_empty() {
        entries.push(current.join("\n"));
    }
    entries
}

fn strip_optional_code_fence(text: &str) -> String {
    let trimmed = text.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }
    let mut lines = trimmed.lines();
    let _opening = lines.next();
    let body = lines.collect::<Vec<_>>();
    if body.last().is_some_and(|line| line.trim() == "```") {
        return body[..body.len().saturating_sub(1)].join("\n");
    }
    trimmed.to_string()
}

fn strip_bullet_marker(line: &str) -> Option<&str> {
    line.strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
        .or_else(|| {
            line.split_once(". ").and_then(|(prefix, rest)| {
                prefix.chars().all(|ch| ch.is_ascii_digit()).then_some(rest)
            })
        })
        .map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::{build_session_episodic_capture_prompt, parse_session_episodic_capture_entries};

    #[test]
    fn prompt_keeps_append_only_capture_constraints() {
        let prompt = build_session_episodic_capture_prompt("user> remember the rollback channel");

        assert!(prompt.contains("append-only episodic daily log"));
        assert!(prompt.contains("Return ONLY Markdown bullet lines"));
        assert!(prompt.contains("remember the rollback channel"));
    }

    #[test]
    fn parser_accepts_bullets_and_continuations() {
        let entries = parse_session_episodic_capture_entries(
            "- User prefers canary deploys\n  keep this for rollback safety\n- Incident moved to pager duty",
        );

        assert_eq!(entries.len(), 2);
        assert!(entries[0].contains("User prefers canary deploys"));
        assert!(entries[0].contains("keep this for rollback safety"));
        assert_eq!(entries[1], "Incident moved to pager duty");
    }

    #[test]
    fn parser_treats_none_as_empty() {
        assert!(parse_session_episodic_capture_entries("NONE").is_empty());
    }
}
