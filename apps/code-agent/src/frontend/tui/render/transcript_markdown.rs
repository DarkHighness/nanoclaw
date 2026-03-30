use super::transcript::{TranscriptEntryKind, line_has_visible_content};
use super::transcript_markdown_blocks::{apply_markdown_prefixes, render_markdown_lines};
use ratatui::text::{Line, Span};

pub(super) use super::transcript_markdown_blocks::{code_span, render_shell_code_block};
pub(super) use super::transcript_markdown_line::render_transcript_body_line;

pub(super) fn render_markdown_body(body: &str, kind: TranscriptEntryKind) -> Vec<Line<'static>> {
    let mut rendered = Vec::new();
    let mut text_chunk = Vec::new();
    let mut is_first_visible = true;
    let mut lines = body.lines();

    while let Some(raw_line) = lines.next() {
        if let Some(language) = raw_line.trim_start().strip_prefix("```") {
            if !text_chunk.is_empty() {
                let chunk = render_markdown_chunk(&text_chunk.join("\n"), kind, is_first_visible);
                if chunk.iter().any(line_has_visible_content) {
                    is_first_visible = false;
                }
                rendered.extend(chunk);
                text_chunk.clear();
            }

            let mut code_lines = Vec::new();
            for code_line in lines.by_ref() {
                if code_line.trim_start().starts_with("```") {
                    break;
                }
                code_lines.push(code_line);
            }
            let block = render_shell_code_block(
                language.trim(),
                &code_lines.join("\n"),
                kind,
                is_first_visible,
            );
            if block.iter().any(line_has_visible_content) {
                is_first_visible = false;
            }
            rendered.extend(block);
            continue;
        }
        text_chunk.push(raw_line);
    }

    if !text_chunk.is_empty() {
        rendered.extend(render_markdown_chunk(
            &text_chunk.join("\n"),
            kind,
            is_first_visible,
        ));
    }

    if rendered.is_empty() {
        vec![Line::from(Span::raw(""))]
    } else {
        rendered
    }
}

fn render_markdown_chunk(
    body: &str,
    kind: TranscriptEntryKind,
    is_first_visible: bool,
) -> Vec<Line<'static>> {
    let mut compact = render_markdown_lines(body);
    apply_markdown_prefixes(&mut compact, kind, !is_first_visible);
    compact
}
