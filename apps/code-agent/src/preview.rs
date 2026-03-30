#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreviewCollapse {
    Head,
    Tail,
    HeadTail,
}

pub(crate) fn collapse_preview_text(
    value: &str,
    max_lines: usize,
    max_columns: usize,
    collapse: PreviewCollapse,
) -> Vec<String> {
    let raw_lines = value.lines().collect::<Vec<_>>();
    if raw_lines.is_empty() {
        return vec!["<empty>".to_string()];
    }

    let clipped_lines = raw_lines
        .into_iter()
        .map(|line| clip_line(line, max_columns))
        .collect::<Vec<_>>();
    collapse_preview_lines(&clipped_lines, max_lines, collapse)
}

pub(crate) fn collapse_preview_lines(
    lines: &[String],
    max_lines: usize,
    collapse: PreviewCollapse,
) -> Vec<String> {
    if lines.is_empty() {
        return Vec::new();
    }

    if lines.len() <= max_lines.max(1) {
        return lines.to_vec();
    }

    match collapse {
        PreviewCollapse::Head => {
            let head = max_lines.saturating_sub(1).max(1);
            let hidden = lines.len().saturating_sub(head);
            let mut preview = lines.iter().take(head).cloned().collect::<Vec<_>>();
            preview.push(omitted_lines_label(hidden));
            preview
        }
        PreviewCollapse::Tail => {
            let tail = max_lines.saturating_sub(1).max(1);
            let hidden = lines.len().saturating_sub(tail);
            let mut preview = Vec::with_capacity(tail + 1);
            preview.push(omitted_lines_label(hidden));
            preview.extend(lines.iter().skip(lines.len().saturating_sub(tail)).cloned());
            preview
        }
        PreviewCollapse::HeadTail => {
            // Keep the omission marker inside the line budget so previews stay
            // compact while still preserving tail context for diffs and output.
            let visible_budget = max_lines.max(3);
            let tail = (visible_budget - 1) / 2;
            let head = visible_budget - tail - 1;
            let hidden = lines.len().saturating_sub(head + tail);
            let mut preview = lines.iter().take(head).cloned().collect::<Vec<_>>();
            preview.push(omitted_lines_label(hidden));
            preview.extend(lines.iter().skip(lines.len().saturating_sub(tail)).cloned());
            preview
        }
    }
}

fn clip_line(line: &str, max_columns: usize) -> String {
    if line.chars().count() > max_columns {
        format!(
            "{}...",
            line.chars()
                .take(max_columns.saturating_sub(3))
                .collect::<String>()
        )
    } else {
        line.to_string()
    }
}

fn omitted_lines_label(hidden: usize) -> String {
    format!("… +{hidden} lines")
}

pub(crate) fn command_output_collapse(
    exit_code: Option<i64>,
    timed_out: bool,
    has_stderr: bool,
) -> PreviewCollapse {
    if timed_out || has_stderr || exit_code.is_some_and(|code| code != 0) {
        PreviewCollapse::Tail
    } else {
        PreviewCollapse::HeadTail
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PreviewCollapse, collapse_preview_lines, collapse_preview_text, command_output_collapse,
    };

    #[test]
    fn head_collapse_keeps_prefix_and_reports_hidden_count() {
        let lines =
            collapse_preview_text("one\ntwo\nthree\nfour\nfive", 4, 80, PreviewCollapse::Head);

        assert_eq!(lines, vec!["one", "two", "three", "… +2 lines"]);
    }

    #[test]
    fn head_tail_collapse_keeps_tail_context() {
        let lines = collapse_preview_text(
            "one\ntwo\nthree\nfour\nfive\nsix",
            4,
            80,
            PreviewCollapse::HeadTail,
        );

        assert_eq!(lines, vec!["one", "two", "… +3 lines", "six"]);
    }

    #[test]
    fn tail_collapse_keeps_suffix_and_reports_hidden_count() {
        let lines =
            collapse_preview_text("one\ntwo\nthree\nfour\nfive", 4, 80, PreviewCollapse::Tail);

        assert_eq!(lines, vec!["… +2 lines", "three", "four", "five"]);
    }

    #[test]
    fn line_collapse_operates_on_prebuilt_lines() {
        let lines = collapse_preview_lines(
            &[
                "alpha".to_string(),
                "beta".to_string(),
                "gamma".to_string(),
                "delta".to_string(),
                "omega".to_string(),
            ],
            4,
            PreviewCollapse::HeadTail,
        );

        assert_eq!(lines, vec!["alpha", "beta", "… +2 lines", "omega"]);
    }

    #[test]
    fn command_output_prefers_tail_for_diagnostics() {
        assert_eq!(
            command_output_collapse(Some(1), false, false),
            PreviewCollapse::Tail
        );
        assert_eq!(
            command_output_collapse(Some(0), false, true),
            PreviewCollapse::Tail
        );
        assert_eq!(
            command_output_collapse(Some(0), false, false),
            PreviewCollapse::HeadTail
        );
    }
}
