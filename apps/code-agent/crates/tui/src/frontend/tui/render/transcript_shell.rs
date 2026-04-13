use super::super::state::{
    TranscriptEntry, TranscriptShellBlockKind, TranscriptShellDetail, TranscriptShellEntry,
    TranscriptShellStatus, TranscriptToolEntry, TranscriptToolHeadlineSubjectKind,
    TranscriptToolStatus, TuiState, preview_text,
};
use super::shared::{
    pending_control_focus_label, pending_control_kind_label, pending_control_reason_label,
};
use super::statusline::status_color;
use super::theme::palette;
use super::transcript::TranscriptEntryKind;
use super::transcript_markdown::{render_markdown_body, render_shell_code_block};
use super::transcript_markdown_blocks::code_span;
use super::transcript_markdown_line::render_transcript_body_line;
use crate::tool_render::{
    ToolCommand, ToolCommandIntent, ToolCompletionState, ToolDetail, ToolDetailBlockKind,
    ToolDetailLabel, ToolReviewItem, ToolReviewItemKind,
};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::time::{Duration, Instant};

const COLLAPSED_SHELL_PREVIEW_DETAIL_LINES: usize = 2;
const SELECTED_TOOL_PREVIEW_DETAIL_LINES: usize = 5;

#[derive(Clone, Debug, Default)]
pub(super) struct RenderedTranscriptCell {
    pub(super) header: Vec<Line<'static>>,
    pub(super) body: Vec<Line<'static>>,
    pub(super) meta: Vec<Line<'static>>,
}

impl RenderedTranscriptCell {
    pub(super) fn with_body(body: Vec<Line<'static>>) -> Self {
        Self {
            header: Vec::new(),
            body,
            meta: Vec::new(),
        }
    }
}

pub(super) fn should_collapse_shell_details(
    entry: &TranscriptEntry,
    show_tool_details: bool,
) -> bool {
    // Keep the default transcript on a single readable timeline. Operators can
    // opt back into the full tool payload stream via `/details`.
    !show_tool_details && entry.is_shell_summary() && hidden_shell_detail_line_count(entry) > 0
}

pub(super) fn should_collapse_tool_details(
    entry: &TranscriptEntry,
    show_tool_details: bool,
) -> bool {
    !show_tool_details && hidden_tool_detail_line_count(entry) > 0
}

pub(super) fn render_collapsed_tool_entry(
    entry: &TranscriptEntry,
    marker: &str,
    accent: Color,
    kind: TranscriptEntryKind,
    animation_frame: Option<u128>,
    selected: bool,
) -> RenderedTranscriptCell {
    let tool = entry
        .tool_entry()
        .expect("collapsed tool entries require structured tool payloads");
    let preview_line_count = if selected {
        SELECTED_TOOL_PREVIEW_DETAIL_LINES
    } else {
        COLLAPSED_SHELL_PREVIEW_DETAIL_LINES
    };
    let preview = tool.preview_with_detail_lines(preview_line_count);
    let hidden_line_count = hidden_tool_detail_line_count_with_limit(entry, preview_line_count);
    let mut cell = render_tool_entry_sections(&preview, marker, kind, animation_frame);
    if hidden_line_count > 0 {
        cell.meta
            .push(hidden_detail_hint_line(hidden_line_count, kind));
    }
    let _ = (marker, accent);
    prefix_tool_marker(&mut cell.header, &preview, kind, animation_frame);
    cell
}

pub(super) fn render_collapsed_shell_summary(
    entry: &TranscriptEntry,
    marker: &str,
    accent: Color,
    kind: TranscriptEntryKind,
    animation_frame: Option<u128>,
) -> RenderedTranscriptCell {
    let summary = entry
        .shell_summary()
        .expect("collapsed shell summaries require structured details");
    let preview_summary = summary.preview_with_detail_lines(COLLAPSED_SHELL_PREVIEW_DETAIL_LINES);
    let hidden_line_count = hidden_shell_detail_line_count(entry);
    let mut cell = render_shell_summary_sections(&preview_summary, marker, kind, animation_frame);
    if hidden_line_count > 0 {
        cell.meta
            .push(hidden_detail_hint_line(hidden_line_count, kind));
    }
    prefix_transcript_marker(&mut cell.header, marker, accent, kind);
    cell
}

fn hidden_detail_hint_line(hidden_line_count: usize, kind: TranscriptEntryKind) -> Line<'static> {
    Line::from(vec![
        transcript_continuation_prefix(kind),
        Span::styled(
            format!(
                "{} hidden line{} · /details",
                hidden_line_count,
                if hidden_line_count == 1 { "" } else { "s" }
            ),
            Style::default().fg(palette().subtle),
        ),
    ])
}

fn hidden_tool_detail_line_count_with_limit(
    entry: &TranscriptEntry,
    max_detail_lines: usize,
) -> usize {
    entry
        .tool_entry()
        .map(|tool| tool.serialized_lines().len().saturating_sub(1))
        .unwrap_or_default()
        .saturating_sub(max_detail_lines)
}

fn hidden_tool_detail_line_count(entry: &TranscriptEntry) -> usize {
    hidden_tool_detail_line_count_with_limit(entry, COLLAPSED_SHELL_PREVIEW_DETAIL_LINES)
}

fn hidden_shell_detail_line_count(entry: &TranscriptEntry) -> usize {
    entry
        .shell_summary()
        .map(|summary| summary.serialized_lines().len().saturating_sub(1))
        .unwrap_or_default()
        .saturating_sub(COLLAPSED_SHELL_PREVIEW_DETAIL_LINES)
}

pub(super) fn render_shell_summary_body(
    body: &str,
    marker: &str,
    kind: TranscriptEntryKind,
    animation_frame: Option<u128>,
) -> Vec<Line<'static>> {
    let mut rendered = Vec::new();
    let mut first_visible = true;
    let mut lines = body.lines();

    while let Some(raw_line) = lines.next() {
        let trimmed = raw_line.trim_start();
        if let Some(language) = trimmed.strip_prefix("```") {
            let mut code_lines = Vec::new();
            for code_line in lines.by_ref() {
                if code_line.trim_start().starts_with("```") {
                    break;
                }
                code_lines.push(code_line);
            }
            let code = code_lines.join("\n");
            let block = render_shell_code_block(language.trim(), &code, kind, first_visible);
            if block.iter().any(line_has_visible_content) {
                first_visible = false;
            }
            rendered.extend(block);
            continue;
        }

        if first_visible
            && let Some(animated) =
                render_animated_shell_status_line(None, raw_line, marker, kind, animation_frame)
        {
            rendered.push(animated);
        } else {
            rendered.push(render_transcript_body_line(
                raw_line,
                marker,
                kind,
                false,
                first_visible,
            ));
        }
        if !raw_line.trim().is_empty() {
            first_visible = false;
        }
    }

    if rendered.is_empty() {
        rendered.push(Line::from(Span::raw("")));
    }

    rendered
}

pub(super) fn render_shell_summary_sections(
    summary: &TranscriptShellEntry,
    marker: &str,
    kind: TranscriptEntryKind,
    animation_frame: Option<u128>,
) -> RenderedTranscriptCell {
    let mut header = Vec::new();
    if let Some(animated) = render_animated_shell_status_line(
        summary.status,
        &summary.headline,
        marker,
        kind,
        animation_frame,
    ) {
        header.push(animated);
    } else if !summary.headline.trim().is_empty() {
        header.push(render_transcript_body_line(
            &summary.headline,
            marker,
            kind,
            false,
            true,
        ));
    }

    let mut body = Vec::new();
    for detail in &summary.detail_lines {
        body.extend(render_shell_detail(detail, kind));
    }

    RenderedTranscriptCell {
        header,
        body,
        meta: Vec::new(),
    }
}

pub(super) fn render_tool_entry_sections(
    entry: &TranscriptToolEntry,
    marker: &str,
    kind: TranscriptEntryKind,
    animation_frame: Option<u128>,
) -> RenderedTranscriptCell {
    let mut header = Vec::new();
    if let Some(animated) = render_animated_tool_status_line(entry, marker, kind, animation_frame) {
        header.push(animated);
    } else if !entry.headline.trim().is_empty() {
        header.push(render_tool_status_line(entry));
    }

    let mut body = Vec::new();
    let mut meta = Vec::new();
    for detail in &entry.detail_lines {
        match detail {
            ToolDetail::ActionHint {
                key_hint,
                label,
                detail,
            } => meta.push(detail_line(
                false,
                labeled_detail_spans(
                    "action",
                    palette().assistant,
                    tool_action_spans(key_hint, label, detail.as_deref()),
                ),
            )),
            _ => body.extend(render_tool_detail(detail, kind)),
        }
    }

    RenderedTranscriptCell { header, body, meta }
}

fn render_animated_shell_status_line(
    status: Option<TranscriptShellStatus>,
    raw_line: &str,
    marker: &str,
    kind: TranscriptEntryKind,
    animation_frame: Option<u128>,
) -> Option<Line<'static>> {
    let frame_ms = animation_frame?;
    let (status_label, remainder, accent) = status
        .and_then(|status| shell_status_phrase(status, raw_line))
        .or_else(|| legacy_shell_status_phrase(raw_line))?;
    let mut spans = animated_status_phrase_spans(status_label, frame_ms, accent);
    if !remainder.is_empty() {
        spans.push(Span::styled(
            remainder.to_string(),
            transcript_body_style(marker, kind, raw_line),
        ));
    }
    Some(Line::from(spans))
}

fn render_animated_tool_status_line(
    entry: &TranscriptToolEntry,
    marker: &str,
    kind: TranscriptEntryKind,
    animation_frame: Option<u128>,
) -> Option<Line<'static>> {
    let frame_ms = animation_frame?;
    let _ = (marker, kind);
    let headline = tool_headline(entry);
    if !matches!(
        entry.status,
        TranscriptToolStatus::Running
            | TranscriptToolStatus::Requested
            | TranscriptToolStatus::WaitingApproval
    ) {
        return Some(render_tool_status_line(entry));
    }

    let mut spans = animated_status_phrase_spans(headline.verb, frame_ms, headline.accent);
    if let Some(subject) = headline.subject {
        spans.push(Span::styled(" ", Style::default().fg(palette().subtle)));
        spans.extend(render_tool_subject_spans(subject));
    }
    Some(Line::from(spans))
}

pub(super) fn prefix_transcript_marker(
    lines: &mut [Line<'static>],
    marker: &str,
    accent: Color,
    kind: TranscriptEntryKind,
) {
    let index = lines
        .iter()
        .position(line_has_visible_content)
        .unwrap_or_default();
    let mut spans = vec![
        Span::styled(
            marker.to_string(),
            transcript_marker_style(marker, accent, kind),
        ),
        Span::raw(" "),
    ];
    spans.extend(lines[index].spans.clone());
    lines[index] = Line::from(spans);
}

pub(super) fn prefix_tool_marker(
    lines: &mut [Line<'static>],
    entry: &TranscriptToolEntry,
    kind: TranscriptEntryKind,
    animation_frame: Option<u128>,
) {
    let index = lines
        .iter()
        .position(line_has_visible_content)
        .unwrap_or_default();
    let mut spans = vec![
        tool_marker_span(entry, kind, animation_frame),
        Span::raw(" "),
    ];
    spans.extend(lines[index].spans.clone());
    lines[index] = Line::from(spans);
}

pub(super) fn line_has_visible_content(line: &Line<'static>) -> bool {
    !line_to_plain_text(line).trim().is_empty()
}

pub(super) fn line_to_plain_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

pub(super) fn transcript_continuation_prefix(kind: TranscriptEntryKind) -> Span<'static> {
    match kind {
        TranscriptEntryKind::ShellSummary
        | TranscriptEntryKind::SuccessSummary
        | TranscriptEntryKind::ErrorSummary
        | TranscriptEntryKind::WarningSummary => {
            Span::styled("    ", Style::default().fg(palette().subtle))
        }
        _ => Span::raw("  "),
    }
}

pub(super) fn transcript_body_style(marker: &str, kind: TranscriptEntryKind, _line: &str) -> Style {
    let style = match kind {
        TranscriptEntryKind::UserPrompt | TranscriptEntryKind::AssistantMessage => {
            Style::default().fg(palette().text)
        }
        TranscriptEntryKind::ShellSummary => Style::default().fg(palette().muted),
        TranscriptEntryKind::SuccessSummary => Style::default().fg(palette().assistant),
        TranscriptEntryKind::ErrorSummary => Style::default().fg(palette().error),
        TranscriptEntryKind::WarningSummary => Style::default().fg(palette().warn),
    };

    if marker == "›" {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

fn tool_marker_span(
    entry: &TranscriptToolEntry,
    kind: TranscriptEntryKind,
    animation_frame: Option<u128>,
) -> Span<'static> {
    let marker = entry.marker();
    let accent = tool_status_accent(entry.status, entry.completion);
    if entry.status == TranscriptToolStatus::Running
        && let Some(frame_ms) = animation_frame
    {
        return animated_tool_marker(marker, frame_ms);
    }
    Span::styled(
        marker.to_string(),
        transcript_marker_style(marker, accent, kind).add_modifier(Modifier::BOLD),
    )
}

fn animated_tool_marker(marker: &str, frame_ms: u128) -> Span<'static> {
    let phase = ((frame_ms / 160) % 6) as usize;
    let (color, modifier) = match phase {
        0 => (palette().subtle, Modifier::empty()),
        1 => (palette().muted, Modifier::empty()),
        2 => (palette().text, Modifier::empty()),
        3 => (palette().assistant, Modifier::BOLD),
        4 => (palette().header, Modifier::BOLD),
        _ => (palette().user, Modifier::BOLD),
    };
    Span::styled(
        marker.to_string(),
        Style::default().fg(color).add_modifier(modifier),
    )
}

fn render_shell_detail(
    detail: &TranscriptShellDetail,
    kind: TranscriptEntryKind,
) -> Vec<Line<'static>> {
    match detail {
        TranscriptShellDetail::Command(command) => vec![detail_line(
            false,
            vec![Span::styled(
                command.clone(),
                Style::default().fg(palette().user),
            )],
        )],
        TranscriptShellDetail::Meta(text) => vec![detail_line(
            false,
            vec![Span::styled(text.clone(), shell_meta_style(text))],
        )],
        TranscriptShellDetail::TextBlock(lines) => render_shell_text_block(lines, kind),
        TranscriptShellDetail::NamedBlock {
            label,
            kind: block_kind,
            lines,
        } => render_named_shell_block(label, *block_kind, lines),
        TranscriptShellDetail::Raw { text, continuation } => vec![detail_line(
            *continuation,
            vec![Span::styled(
                text.clone(),
                Style::default().fg(palette().muted),
            )],
        )],
    }
}

fn render_tool_detail(detail: &ToolDetail, kind: TranscriptEntryKind) -> Vec<Line<'static>> {
    match detail {
        ToolDetail::Command(command) => render_command_detail(command),
        ToolDetail::Meta(text) => vec![detail_line(
            false,
            labeled_detail_spans(
                ToolDetailLabel::Note.as_str(),
                tool_detail_label_color(ToolDetailLabel::Note),
                vec![Span::styled(text.clone(), shell_meta_style(text))],
            ),
        )],
        ToolDetail::LabeledValue { label, value } => vec![detail_line(
            false,
            labeled_detail_spans(
                label.as_str(),
                tool_detail_label_color(*label),
                vec![Span::styled(
                    value.clone(),
                    tool_detail_value_style(*label, value),
                )],
            ),
        )],
        ToolDetail::LabeledBlock { label, lines } => render_labeled_tool_block(label, lines, kind),
        ToolDetail::TextBlock(lines) => render_tool_text_block(lines, kind),
        ToolDetail::NamedBlock {
            label,
            kind: block_kind,
            lines,
        } => render_named_tool_block(label, *block_kind, lines),
        ToolDetail::ActionHint { .. } => Vec::new(),
    }
}

fn render_command_detail(command: &ToolCommand) -> Vec<Line<'static>> {
    match command.intent {
        ToolCommandIntent::Explore => {
            let summaries = command.summary_lines();
            if let Some((first, rest)) = summaries.split_first() {
                let mut rendered = vec![detail_line(
                    false,
                    vec![Span::styled(
                        first.clone(),
                        Style::default().fg(palette().text),
                    )],
                )];
                rendered.extend(rest.iter().map(|summary| {
                    detail_line(
                        true,
                        vec![Span::styled(
                            summary.clone(),
                            Style::default().fg(palette().text),
                        )],
                    )
                }));
                rendered
            } else {
                vec![detail_line(
                    false,
                    vec![Span::styled(
                        command.preview_line(),
                        Style::default().fg(palette().text),
                    )],
                )]
            }
        }
        ToolCommandIntent::Execute => vec![detail_line(
            false,
            labeled_detail_spans(
                "command",
                palette().accent,
                shell_command_spans(&command.raw),
            ),
        )],
    }
}

fn render_shell_text_block(lines: &[String], kind: TranscriptEntryKind) -> Vec<Line<'static>> {
    render_markdown_detail_block(&lines.join("\n"), kind)
}

fn render_named_shell_block(
    label: &str,
    block_kind: TranscriptShellBlockKind,
    lines: &[String],
) -> Vec<Line<'static>> {
    let mut rendered = vec![detail_line(
        false,
        vec![Span::styled(
            label.to_string(),
            shell_block_label_style(block_kind),
        )],
    )];

    rendered.extend(lines.iter().map(|line| match block_kind {
        TranscriptShellBlockKind::Diff => detail_line(true, vec![code_span(line)]),
        TranscriptShellBlockKind::Stderr => detail_line(
            true,
            vec![Span::styled(
                line.clone(),
                Style::default().fg(palette().error),
            )],
        ),
        TranscriptShellBlockKind::Stdout => detail_line(
            true,
            vec![Span::styled(
                line.clone(),
                Style::default().fg(palette().text),
            )],
        ),
    }));

    rendered
}

fn render_named_tool_block(
    label: &str,
    block_kind: ToolDetailBlockKind,
    lines: &[String],
) -> Vec<Line<'static>> {
    let block_kind = TranscriptShellBlockKind::from(block_kind);
    let mut rendered = Vec::new();
    if let Some((first, rest)) = lines.split_first() {
        rendered.push(detail_line(
            false,
            labeled_detail_spans(
                label,
                shell_block_label_style(block_kind)
                    .fg
                    .unwrap_or(palette().text),
                vec![tool_block_span(first, block_kind)],
            ),
        ));
        rendered.extend(
            rest.iter()
                .map(|line| detail_line(true, vec![tool_block_span(line, block_kind)])),
        );
    } else {
        rendered.push(detail_line(
            false,
            labeled_detail_spans(
                label,
                shell_block_label_style(block_kind)
                    .fg
                    .unwrap_or(palette().text),
                Vec::new(),
            ),
        ));
    }
    rendered
}

fn render_labeled_tool_block(
    label: &ToolDetailLabel,
    lines: &[String],
    kind: TranscriptEntryKind,
) -> Vec<Line<'static>> {
    render_markdown_labeled_detail_block(
        label.as_str(),
        tool_detail_label_color(*label),
        &lines.join("\n"),
        kind,
    )
}

fn render_tool_status_line(entry: &TranscriptToolEntry) -> Line<'static> {
    let headline = tool_headline(entry);
    let mut spans = vec![Span::styled(
        headline.verb.to_string(),
        Style::default()
            .fg(headline.accent)
            .add_modifier(Modifier::BOLD),
    )];
    if let Some(subject) = headline.subject {
        spans.push(Span::styled(" ", Style::default().fg(palette().subtle)));
        spans.extend(render_tool_subject_spans(subject));
    }
    Line::from(spans)
}

#[derive(Clone, Copy)]
struct ToolHeadline<'a> {
    verb: &'static str,
    accent: Color,
    subject: Option<ToolHeadlineSubject<'a>>,
}

#[derive(Clone, Copy)]
enum ToolHeadlineSubject<'a> {
    Command(&'a ToolCommand),
    Text(&'a str),
}

fn tool_headline(entry: &TranscriptToolEntry) -> ToolHeadline<'_> {
    let accent = tool_status_accent(entry.status, entry.completion);
    ToolHeadline {
        verb: entry.headline_prefix(),
        accent,
        subject: match entry.headline_subject_kind() {
            TranscriptToolHeadlineSubjectKind::None => None,
            TranscriptToolHeadlineSubjectKind::Command => {
                first_tool_command(entry).map(ToolHeadlineSubject::Command)
            }
            TranscriptToolHeadlineSubjectKind::ToolName => {
                Some(ToolHeadlineSubject::Text(&entry.tool_name))
            }
        },
    }
}

fn first_tool_command(entry: &TranscriptToolEntry) -> Option<&ToolCommand> {
    entry.detail_lines.iter().find_map(|detail| match detail {
        ToolDetail::Command(command) => Some(command),
        _ => None,
    })
}

fn render_tool_subject_spans(subject: ToolHeadlineSubject<'_>) -> Vec<Span<'static>> {
    match subject {
        ToolHeadlineSubject::Command(command) => shell_command_spans(&command.raw),
        ToolHeadlineSubject::Text(text) => vec![Span::styled(
            text.to_string(),
            Style::default()
                .fg(palette().header)
                .add_modifier(Modifier::BOLD),
        )],
    }
}

pub(super) fn render_tool_review_preview_lines(item: &ToolReviewItem) -> Vec<Line<'static>> {
    item.preview_lines
        .iter()
        .map(|line| {
            let spans = match item.preview_kind {
                ToolReviewItemKind::Command => {
                    if let Some(command) = line.strip_prefix("$ ") {
                        let mut spans = vec![
                            Span::styled("$", Style::default().fg(palette().accent)),
                            Span::styled(" ", Style::default().fg(palette().subtle)),
                        ];
                        spans.extend(shell_command_spans(command));
                        spans
                    } else {
                        shell_command_spans(line)
                    }
                }
                ToolReviewItemKind::Stdout => vec![Span::styled(
                    line.clone(),
                    Style::default().fg(palette().text),
                )],
                ToolReviewItemKind::Stderr => vec![Span::styled(
                    line.clone(),
                    Style::default().fg(palette().error),
                )],
                ToolReviewItemKind::Diff => {
                    vec![tool_block_span(line, TranscriptShellBlockKind::Diff)]
                }
                ToolReviewItemKind::Neutral => vec![Span::styled(
                    line.clone(),
                    Style::default().fg(palette().text),
                )],
            };
            Line::from(spans)
        })
        .collect()
}

fn detail_line(continuation: bool, mut spans: Vec<Span<'static>>) -> Line<'static> {
    let prefix = if continuation { "    " } else { "  └ " };
    spans.insert(
        0,
        Span::styled(prefix.to_string(), Style::default().fg(palette().subtle)),
    );
    Line::from(spans)
}

fn labeled_detail_spans(
    label: &str,
    label_color: Color,
    mut body: Vec<Span<'static>>,
) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(
        format!("{label:<8}"),
        Style::default()
            .fg(label_color)
            .add_modifier(Modifier::BOLD),
    )];
    if !body.is_empty() {
        spans.push(Span::raw(" "));
        spans.append(&mut body);
    }
    spans
}

fn render_tool_text_block(lines: &[String], kind: TranscriptEntryKind) -> Vec<Line<'static>> {
    render_markdown_labeled_detail_block("Output", palette().muted, &lines.join("\n"), kind)
}

fn render_markdown_detail_block(body: &str, kind: TranscriptEntryKind) -> Vec<Line<'static>> {
    let mut rendered = render_markdown_body(body, kind);
    if let Some(index) = rendered.iter().position(line_has_visible_content) {
        let body_spans = rendered[index].spans.clone();
        rendered[index] = detail_line(false, body_spans);
    }
    rendered
}

fn render_markdown_labeled_detail_block(
    label: &str,
    label_color: Color,
    body: &str,
    kind: TranscriptEntryKind,
) -> Vec<Line<'static>> {
    let mut rendered = render_markdown_body(body, kind);
    let Some(index) = rendered.iter().position(line_has_visible_content) else {
        return vec![detail_line(
            false,
            labeled_detail_spans(label, label_color, Vec::new()),
        )];
    };

    let body_spans = rendered[index].spans.clone();
    rendered[index] = detail_line(false, labeled_detail_spans(label, label_color, body_spans));
    rendered
}

fn tool_action_spans(key_hint: &str, label: &str, detail: Option<&str>) -> Vec<Span<'static>> {
    let mut spans = vec![
        Span::styled(key_hint.to_string(), Style::default().fg(palette().accent)),
        Span::styled(format!(" {label}"), Style::default().fg(palette().muted)),
    ];
    if let Some(detail) = detail.filter(|detail| !detail.trim().is_empty()) {
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
        spans.push(Span::styled(
            detail.to_string(),
            Style::default().fg(palette().text),
        ));
    }
    spans
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellTokenKind {
    Whitespace,
    Command,
    Flag,
    String,
    Operator,
    Env,
    Path,
    Text,
}

pub(super) fn shell_command_spans(command: &str) -> Vec<Span<'static>> {
    tokenize_shell_command(command)
        .into_iter()
        .map(|(token, kind)| Span::styled(token, shell_token_style(kind)))
        .collect()
}

fn shell_token_style(kind: ShellTokenKind) -> Style {
    match kind {
        ShellTokenKind::Whitespace => Style::default().fg(palette().subtle),
        ShellTokenKind::Command => Style::default()
            .fg(palette().header)
            .add_modifier(Modifier::BOLD),
        ShellTokenKind::Flag => Style::default().fg(palette().accent),
        ShellTokenKind::String => Style::default().fg(palette().assistant),
        ShellTokenKind::Operator => Style::default()
            .fg(palette().subtle)
            .add_modifier(Modifier::BOLD),
        ShellTokenKind::Env => Style::default().fg(palette().user),
        ShellTokenKind::Path => Style::default().fg(palette().text),
        ShellTokenKind::Text => Style::default().fg(palette().muted),
    }
}

fn tokenize_shell_command(command: &str) -> Vec<(String, ShellTokenKind)> {
    let chars = command.chars().collect::<Vec<_>>();
    let mut tokens = Vec::new();
    let mut index = 0usize;
    let mut expect_command = true;

    while index < chars.len() {
        let ch = chars[index];
        if ch.is_whitespace() {
            let start = index;
            while index < chars.len() && chars[index].is_whitespace() {
                index += 1;
            }
            tokens.push((
                chars[start..index].iter().collect(),
                ShellTokenKind::Whitespace,
            ));
            continue;
        }

        if is_shell_operator_char(ch) {
            let start = index;
            index += 1;
            while index < chars.len() && is_shell_operator_char(chars[index]) {
                index += 1;
            }
            let token = chars[start..index].iter().collect::<String>();
            if resets_shell_command_position(&token) {
                expect_command = true;
            }
            tokens.push((token, ShellTokenKind::Operator));
            continue;
        }

        let start = index;
        let kind = if matches!(ch, '\'' | '"') {
            let quote = ch;
            index += 1;
            while index < chars.len() {
                let current = chars[index];
                index += 1;
                if current == quote {
                    break;
                }
                if quote == '"' && current == '\\' && index < chars.len() {
                    index += 1;
                }
            }
            ShellTokenKind::String
        } else {
            while index < chars.len()
                && !chars[index].is_whitespace()
                && !is_shell_operator_char(chars[index])
            {
                index += 1;
            }
            classify_shell_word(
                &chars[start..index].iter().collect::<String>(),
                expect_command,
            )
        };
        let token = chars[start..index].iter().collect::<String>();
        if !matches!(kind, ShellTokenKind::Whitespace | ShellTokenKind::Operator)
            && !matches!(kind, ShellTokenKind::Env)
        {
            expect_command = false;
        }
        tokens.push((token, kind));
    }

    tokens
}

fn classify_shell_word(token: &str, expect_command: bool) -> ShellTokenKind {
    if expect_command && token.contains('=') && !token.starts_with('-') {
        return ShellTokenKind::Env;
    }
    if expect_command {
        return ShellTokenKind::Command;
    }
    if token.starts_with('-') {
        return ShellTokenKind::Flag;
    }
    if token.contains('/') || token.starts_with('.') || token.starts_with('~') {
        return ShellTokenKind::Path;
    }
    ShellTokenKind::Text
}

fn is_shell_operator_char(ch: char) -> bool {
    matches!(ch, '|' | '&' | ';' | '(' | ')' | '<' | '>')
}

fn resets_shell_command_position(token: &str) -> bool {
    matches!(token, "|" | "||" | "&&" | ";")
}

fn tool_detail_label_color(label: ToolDetailLabel) -> Color {
    match label {
        ToolDetailLabel::Intent | ToolDetailLabel::Context => palette().accent,
        ToolDetailLabel::Effect => palette().assistant,
        ToolDetailLabel::Files | ToolDetailLabel::Snapshot => palette().header,
        ToolDetailLabel::Result | ToolDetailLabel::Reason => palette().warn,
        ToolDetailLabel::Origin | ToolDetailLabel::State | ToolDetailLabel::Note => {
            palette().subtle
        }
        ToolDetailLabel::Session | ToolDetailLabel::Output => palette().muted,
    }
}

fn tool_detail_value_style(label: ToolDetailLabel, value: &str) -> Style {
    match label {
        ToolDetailLabel::Result => shell_meta_style(value),
        ToolDetailLabel::Files
        | ToolDetailLabel::Effect
        | ToolDetailLabel::Intent
        | ToolDetailLabel::Context
        | ToolDetailLabel::Snapshot
        | ToolDetailLabel::Output => Style::default().fg(palette().text),
        _ => Style::default().fg(palette().muted),
    }
}

fn tool_block_span(line: &str, kind: TranscriptShellBlockKind) -> Span<'static> {
    match kind {
        TranscriptShellBlockKind::Diff => code_span(line),
        TranscriptShellBlockKind::Stderr => {
            Span::styled(line.to_string(), Style::default().fg(palette().error))
        }
        TranscriptShellBlockKind::Stdout => {
            Span::styled(line.to_string(), Style::default().fg(palette().text))
        }
    }
}

fn shell_meta_style(text: &str) -> Style {
    if let Some(exit_code) = text
        .strip_prefix("exit ")
        .and_then(|value| value.parse::<i64>().ok())
    {
        if exit_code == 0 {
            return Style::default().fg(palette().assistant);
        }
        return Style::default().fg(palette().error);
    }
    if text == "timed out" {
        return Style::default().fg(palette().warn);
    }
    Style::default().fg(palette().muted)
}

fn shell_block_label_style(kind: TranscriptShellBlockKind) -> Style {
    match kind {
        TranscriptShellBlockKind::Stdout => Style::default()
            .fg(palette().assistant)
            .add_modifier(Modifier::BOLD),
        TranscriptShellBlockKind::Stderr => Style::default()
            .fg(palette().error)
            .add_modifier(Modifier::BOLD),
        TranscriptShellBlockKind::Diff => Style::default()
            .fg(palette().user)
            .add_modifier(Modifier::BOLD),
    }
}

fn transcript_marker_style(marker: &str, accent: Color, kind: TranscriptEntryKind) -> Style {
    let color = match kind {
        TranscriptEntryKind::AssistantMessage => palette().muted,
        TranscriptEntryKind::UserPrompt => palette().user,
        _ => accent,
    };
    let style = Style::default().fg(color);
    if matches!(marker, "›" | "✗" | "✔") {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

pub(super) fn live_progress_lines(state: &TuiState) -> Vec<Line<'static>> {
    if state.turn_running {
        let frame_time = Instant::now();
        let elapsed = state
            .turn_started_at
            .map(|started| started.elapsed())
            .unwrap_or_default();
        let status = live_progress_summary(state);
        let mut spans = vec![
            Span::styled(
                progress_marker(state),
                Style::default()
                    .fg(status_color(state))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ];
        let mut progress_label = preview_text(&status, 56);
        if let Some(tool_label) = live_tool_progress_label(state) {
            progress_label.push_str(" · ");
            progress_label.push_str(&tool_label);
        }
        spans.extend(animated_progress_text_spans(
            &progress_label,
            animation_frame_ms(state.turn_started_at.unwrap_or(frame_time), frame_time),
        ));
        if state.session.queued_commands > 0 && state.pending_control_picker.is_none() {
            spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
            spans.push(Span::styled(
                if state.active_tool_cells.is_empty() {
                    format!("{} queued", state.session.queued_commands)
                } else if state.active_tool_cells.len() == 1 {
                    format!(
                        "{} queued behind current tool",
                        state.session.queued_commands
                    )
                } else {
                    format!("{} queued behind live tools", state.session.queued_commands)
                },
                Style::default().fg(palette().muted),
            ));
        }
        spans.push(Span::styled(
            format!(" ({} · esc to interrupt)", format_elapsed_duration(elapsed)),
            Style::default().fg(palette().muted),
        ));
        vec![Line::from(spans)]
    } else if state.pending_control_picker.is_none() && state.session.queued_commands > 0 {
        vec![Line::from(vec![
            Span::styled(
                "+",
                Style::default()
                    .fg(palette().warn)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                format!("{} queued command(s)", state.session.queued_commands),
                Style::default().fg(palette().muted),
            ),
        ])]
    } else {
        Vec::new()
    }
}

pub(super) fn pending_control_timeline_entry(state: &TuiState) -> Option<TranscriptEntry> {
    let timeline = pending_control_timeline(state)?;
    let mut detail_lines = Vec::new();
    if timeline.older_hidden_count > 0 {
        detail_lines.push(TranscriptShellDetail::Raw {
            text: format!("{} older pending", timeline.older_hidden_count),
            continuation: false,
        });
    }
    detail_lines.extend(timeline.recent.iter().map(pending_control_timeline_detail));
    if let Some(hint) = pending_control_action_hint(state) {
        detail_lines.push(TranscriptShellDetail::Meta(hint));
    }
    Some(TranscriptEntry::shell_summary_status_details(
        TranscriptShellStatus::Queued,
        format!("Queued Follow-ups · {}", state.pending_controls.len()),
        detail_lines,
    ))
}

pub(super) fn pending_control_picker_bridge_entry(state: &TuiState) -> Option<TranscriptEntry> {
    pending_control_picker_bridge_label(state).map(|label| {
        TranscriptEntry::shell_summary_status_details(
            TranscriptShellStatus::Queued,
            label,
            Vec::new(),
        )
    })
}

pub(super) fn pending_control_embedded_lines(
    state: &TuiState,
    animation_frame: Option<u128>,
) -> Option<Vec<Line<'static>>> {
    let timeline = pending_control_timeline(state)?;
    let mut lines = render_shell_summary_body(
        &format!("Queued Follow-ups · {}", state.pending_controls.len()),
        "•",
        TranscriptEntryKind::ShellSummary,
        animation_frame,
    )
    .into_iter()
    .map(|line| {
        let mut spans = vec![transcript_continuation_prefix(
            TranscriptEntryKind::ShellSummary,
        )];
        spans.extend(line.spans);
        Line::from(spans)
    })
    .collect::<Vec<_>>();
    if timeline.older_hidden_count > 0 {
        lines.push(Line::from(vec![
            transcript_continuation_prefix(TranscriptEntryKind::ShellSummary),
            Span::styled("  └ ", Style::default().fg(palette().subtle)),
            Span::styled(
                format!("{} older pending", timeline.older_hidden_count),
                Style::default().fg(palette().muted),
            ),
        ]));
    }
    lines.extend(
        timeline
            .recent
            .iter()
            .map(render_pending_control_embedded_detail),
    );
    if let Some(hint) = pending_control_action_hint(state) {
        lines.push(Line::from(vec![
            transcript_continuation_prefix(TranscriptEntryKind::ShellSummary),
            Span::styled("  └ ", Style::default().fg(palette().subtle)),
            Span::styled(hint, Style::default().fg(palette().muted)),
        ]));
    }
    Some(lines)
}

pub(super) fn pending_control_picker_embedded_lines(
    state: &TuiState,
    animation_frame: Option<u128>,
) -> Option<Vec<Line<'static>>> {
    let label = pending_control_picker_bridge_label(state)?;
    Some(
        render_shell_summary_body(
            &label,
            "•",
            TranscriptEntryKind::ShellSummary,
            animation_frame,
        )
        .into_iter()
        .map(|line| {
            let mut spans = vec![transcript_continuation_prefix(
                TranscriptEntryKind::ShellSummary,
            )];
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect(),
    )
}

struct PendingControlTimeline {
    older_hidden_count: usize,
    recent: Vec<PendingControlTimelineItem>,
}

struct PendingControlTimelineItem {
    relative_label: &'static str,
    kind: PendingControlKind,
    preview: String,
    reason: Option<String>,
    editing: bool,
}

fn render_pending_control_embedded_detail(item: &PendingControlTimelineItem) -> Line<'static> {
    let (kind_label, kind_color) = pending_control_timeline_kind_label(item.kind, item.editing);
    let mut spans = vec![
        transcript_continuation_prefix(TranscriptEntryKind::ShellSummary),
        Span::styled("  └ ", Style::default().fg(palette().subtle)),
    ];
    if !item.editing {
        spans.push(Span::styled(
            format!("{} ", item.relative_label),
            Style::default().fg(palette().muted),
        ));
    }
    spans.extend([
        Span::styled(
            kind_label,
            Style::default().fg(kind_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(palette().subtle)),
        Span::styled(item.preview.clone(), Style::default().fg(palette().text)),
    ]);
    if let Some(reason) = item.reason.as_deref() {
        spans.push(Span::styled(" · ", Style::default().fg(palette().subtle)));
        spans.push(Span::styled(
            reason.to_string(),
            Style::default().fg(palette().muted),
        ));
    }
    Line::from(spans)
}

fn pending_control_timeline_detail(item: &PendingControlTimelineItem) -> TranscriptShellDetail {
    let (kind_label, _) = pending_control_timeline_kind_label(item.kind, item.editing);
    let mut text = if item.editing {
        format!("{} · {}", kind_label, item.preview)
    } else {
        format!("{} {} · {}", item.relative_label, kind_label, item.preview)
    };
    if let Some(reason) = item.reason.as_deref() {
        text.push_str(" · ");
        text.push_str(reason);
    }
    TranscriptShellDetail::Raw {
        text,
        continuation: false,
    }
}

fn pending_control_timeline_kind_label(
    kind: PendingControlKind,
    editing: bool,
) -> (&'static str, Color) {
    match (kind, editing) {
        (PendingControlKind::Prompt, true) => ("Editing Queued Prompt", palette().user),
        (PendingControlKind::Steer, true) => ("Editing Queued Steer", palette().assistant),
        (PendingControlKind::Prompt, false) => ("Queued Prompt", palette().user),
        (PendingControlKind::Steer, false) => ("Queued Steer", palette().assistant),
    }
}

fn pending_control_action_hint(state: &TuiState) -> Option<String> {
    let latest = state.pending_controls.last()?;
    let mut parts = Vec::new();
    if latest.kind == PendingControlKind::Steer && state.turn_running {
        parts.push("Esc send now".to_string());
    }
    parts.push("Alt+T edit latest".to_string());
    parts.push("Alt+↑ queue".to_string());
    Some(parts.join(" · "))
}

fn pending_control_timeline(state: &TuiState) -> Option<PendingControlTimeline> {
    if state.pending_controls.is_empty() || state.pending_control_picker.is_some() {
        return None;
    }

    // Keep the queued-control summary model shared between the standalone block
    // and the embedded tool continuation so both surfaces describe the same
    // runtime-owned queue ordering.
    let total = state.pending_controls.len();
    let mut visible_indices = if total <= 2 {
        (0..total).collect::<Vec<_>>()
    } else {
        vec![total - 2, total - 1]
    };
    if let Some(editing) = state.editing_pending_control.as_ref()
        && let Some(editing_index) = state
            .pending_controls
            .iter()
            .position(|control| control.id == editing.id)
        && !visible_indices.contains(&editing_index)
    {
        // Editing a queued control should keep that exact item visible in the
        // transcript, even when it is older than the normal "latest two"
        // summary window. Otherwise the operator loses the connection between
        // the composer edit state and the runtime-owned pending queue.
        visible_indices = vec![editing_index, total - 1];
        visible_indices.sort_unstable();
    }

    let visible_total = visible_indices.len();
    let recent = visible_indices
        .into_iter()
        .enumerate()
        .map(|(index, control_index)| {
            let control = &state.pending_controls[control_index];
            let relative_label = if visible_total == 1 {
                "next"
            } else if index + 1 == visible_total {
                "latest"
            } else {
                "older"
            };
            PendingControlTimelineItem {
                relative_label,
                kind: control.kind,
                preview: preview_text(&control.preview, 72),
                reason: pending_control_reason_label(control.reason.as_ref())
                    .map(|reason| preview_text(&reason, 28)),
                editing: state
                    .editing_pending_control
                    .as_ref()
                    .is_some_and(|editing| editing.id == control.id),
            }
        })
        .collect();

    Some(PendingControlTimeline {
        older_hidden_count: total.saturating_sub(2),
        recent,
    })
}

fn pending_control_picker_bridge_label(state: &TuiState) -> Option<String> {
    if state.pending_controls.is_empty() || state.pending_control_picker.is_none() {
        return None;
    }
    if let (Some(selected), Some(picker)) = (
        state.selected_pending_control(),
        state.pending_control_picker.as_ref(),
    ) {
        return Some(format!(
            "Queued follow-ups below · selected {} · {}",
            pending_control_kind_label(selected.kind),
            pending_control_focus_label(picker.selected, state.pending_controls.len()),
        ));
    }
    Some(format!(
        "Queued follow-ups below · {}",
        state.pending_controls.len()
    ))
}

pub(super) fn animated_progress_text_spans(text: &str, frame_ms: u128) -> Vec<Span<'static>> {
    animated_emphasis_text_spans(
        text,
        frame_ms,
        palette().header,
        palette().user,
        palette().text,
        palette().assistant,
        palette().muted,
    )
}

fn animated_status_phrase_spans(text: &str, frame_ms: u128, accent: Color) -> Vec<Span<'static>> {
    animated_emphasis_text_spans(
        text,
        frame_ms,
        palette().header,
        accent,
        palette().text,
        accent,
        palette().muted,
    )
}

fn animated_emphasis_text_spans(
    text: &str,
    frame_ms: u128,
    head_color: Color,
    leading_color: Color,
    mid_color: Color,
    trailing_color: Color,
    base_color: Color,
) -> Vec<Span<'static>> {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return vec![Span::raw("")];
    }

    let glow_width = 7usize;
    let head = ((frame_ms / 75) as usize) % (chars.len() + glow_width);
    let head = head as isize - glow_width as isize;

    chars
        .into_iter()
        .enumerate()
        .map(|(index, ch)| {
            if ch.is_whitespace() {
                return Span::styled(ch.to_string(), Style::default().fg(palette().subtle));
            }

            let delta = index as isize - head;
            let (color, modifier) = match delta {
                0 => (head_color, Modifier::BOLD),
                1 => (leading_color, Modifier::BOLD),
                2 | 3 => (mid_color, Modifier::BOLD),
                4 | 5 => (trailing_color, Modifier::empty()),
                _ => (base_color, Modifier::empty()),
            };
            Span::styled(
                ch.to_string(),
                Style::default().fg(color).add_modifier(modifier),
            )
        })
        .collect()
}

pub(super) fn animation_frame_ms(started_at: Instant, now: Instant) -> u128 {
    now.duration_since(started_at).as_millis()
}

pub(super) fn format_elapsed_duration(elapsed: Duration) -> String {
    let total_secs = elapsed.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;

    if hours > 0 {
        format!("{hours}h {minutes:02}m {seconds:02}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

fn live_progress_summary(state: &TuiState) -> String {
    if state.turn_phase == super::super::state::TurnPhase::WaitingApproval {
        "Waiting for approval".to_string()
    } else if !state.status.is_empty() {
        state.status.clone()
    } else {
        "Working".to_string()
    }
}

fn live_tool_progress_label(state: &TuiState) -> Option<String> {
    let running_cells = state
        .active_tool_cells
        .iter()
        .filter(|cell| cell.is_running())
        .collect::<Vec<_>>();
    match running_cells.as_slice() {
        [] => None,
        [active] => Some(tool_progress_label(&active.entry)),
        running_cells => {
            let names = running_cells
                .iter()
                .map(|active| tool_progress_label(&active.entry))
                .collect::<Vec<_>>()
                .join(", ");
            Some(preview_text(
                &format!("{} running tools: {}", running_cells.len(), names),
                40,
            ))
        }
    }
}

fn progress_marker(state: &TuiState) -> &'static str {
    if state.turn_running {
        "•"
    } else if state.session.queued_commands > 0 {
        "+"
    } else {
        "·"
    }
}

fn tool_progress_label(entry: &TranscriptToolEntry) -> String {
    let headline = tool_headline(entry);
    match headline.subject {
        Some(ToolHeadlineSubject::Command(command)) => {
            format!("{} {}", headline.verb, preview_text(&command.raw, 28))
        }
        Some(ToolHeadlineSubject::Text(subject)) => format!("{} {}", headline.verb, subject),
        None => headline.verb.to_string(),
    }
}

pub(super) fn tool_status_accent(
    status: TranscriptToolStatus,
    completion: ToolCompletionState,
) -> Color {
    match status {
        TranscriptToolStatus::Requested | TranscriptToolStatus::WaitingApproval => palette().warn,
        TranscriptToolStatus::Running => palette().user,
        TranscriptToolStatus::Approved => palette().assistant,
        TranscriptToolStatus::Finished => match completion {
            ToolCompletionState::Failure => palette().error,
            ToolCompletionState::Neutral | ToolCompletionState::Success => palette().assistant,
        },
        TranscriptToolStatus::Denied
        | TranscriptToolStatus::Failed
        | TranscriptToolStatus::Cancelled => palette().error,
    }
}

pub(super) fn shell_status_accent(status: Option<TranscriptShellStatus>) -> Color {
    match status {
        Some(TranscriptShellStatus::Queued) => palette().warn,
        Some(TranscriptShellStatus::Running) => palette().user,
        Some(TranscriptShellStatus::Completed) => palette().assistant,
        Some(TranscriptShellStatus::Failed | TranscriptShellStatus::Cancelled) => palette().error,
        None => palette().muted,
    }
}

fn shell_status_phrase(
    status: TranscriptShellStatus,
    line: &str,
) -> Option<(&'static str, &str, Color)> {
    let (phrase, accent) = match status {
        TranscriptShellStatus::Queued => ("Queued", palette().warn),
        TranscriptShellStatus::Running => ("Running", palette().user),
        TranscriptShellStatus::Completed => ("Completed", palette().assistant),
        TranscriptShellStatus::Failed => ("Failed", palette().error),
        TranscriptShellStatus::Cancelled => ("Cancelled", palette().error),
    };
    line.strip_prefix(phrase)
        .map(|remainder| (phrase, remainder, accent))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LegacyShellStatusPhrase {
    AwaitingApproval,
    Requested,
    Queued,
    Running,
    Finished,
    Approved,
    Denied,
}

impl LegacyShellStatusPhrase {
    fn label(self) -> &'static str {
        match self {
            Self::AwaitingApproval => "Awaiting approval",
            Self::Requested => "Requested",
            Self::Queued => "Queued",
            Self::Running => "Running",
            Self::Finished => "Finished",
            Self::Approved => "Approved",
            Self::Denied => "Denied",
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Self::AwaitingApproval => "Awaiting approval for ",
            Self::Requested => "Requested",
            Self::Queued => "Queued",
            Self::Running => "Running",
            Self::Finished => "Finished",
            Self::Approved => "Approved",
            Self::Denied => "Denied",
        }
    }

    fn accent(self) -> Color {
        match self {
            Self::AwaitingApproval | Self::Requested | Self::Queued => palette().warn,
            Self::Running => palette().user,
            Self::Finished | Self::Approved => palette().assistant,
            Self::Denied => palette().error,
        }
    }

    fn parse(line: &str) -> Option<(Self, &str)> {
        [
            Self::AwaitingApproval,
            Self::Requested,
            Self::Queued,
            Self::Running,
            Self::Finished,
            Self::Approved,
            Self::Denied,
        ]
        .into_iter()
        .find_map(|phrase| {
            line.strip_prefix(phrase.prefix())
                .map(|remainder| (phrase, remainder))
        })
    }
}

fn legacy_shell_status_phrase(line: &str) -> Option<(&'static str, &str, Color)> {
    // Legacy transcript previews and raw summary bodies do not carry typed
    // status metadata. Keep a render-only fallback here instead of rebuilding
    // state from strings elsewhere in the TUI.
    LegacyShellStatusPhrase::parse(line)
        .map(|(phrase, remainder)| (phrase.label(), remainder, phrase.accent()))
}
use crate::interaction::PendingControlKind;
