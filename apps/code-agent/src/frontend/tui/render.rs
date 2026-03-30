mod chrome;
mod picker;
mod shell;
mod statusline;
mod theme;
mod transcript;
mod view;
mod welcome;

use super::approval::ApprovalPrompt;
use super::commands::slash_command_hint;
use super::state::TuiState;
use chrome::{
    approval_band_height, approval_preview_lines, build_approval_text, render_approval_band,
    render_composer, should_render_side_rail, side_rail_width,
};
use picker::{build_command_hint_text, command_hint_height, render_command_hint_band};
use ratatui::layout::{Constraint, Direction, Layout, Position};
use ratatui::style::Style;
use ratatui::widgets::Block;
use shell::{bottom_layout_constraints, composer_inner_area, render_main_pane, render_side_rail};
use statusline::render_status_line;
use theme::*;
use unicode_width::UnicodeWidthStr;

pub(crate) fn render(
    frame: &mut ratatui::Frame<'_>,
    state: &TuiState,
    approval: Option<&ApprovalPrompt>,
) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(BG)), area);

    let approval_height = approval.map(approval_band_height);
    let command_hint = approval
        .is_none()
        .then(|| slash_command_hint(&state.input, state.command_completion_index))
        .flatten();
    let command_hint_height = command_hint.as_ref().map(command_hint_height);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints(bottom_layout_constraints(
            approval_height,
            command_hint_height,
        ))
        .split(area);
    let mut next_index = 0;
    let main_area = vertical[next_index];
    next_index += 1;
    let approval_area = approval_height.map(|_| {
        let area = vertical[next_index];
        next_index += 1;
        area
    });
    let command_hint_area = command_hint_height.map(|_| {
        let area = vertical[next_index];
        next_index += 1;
        area
    });
    let composer_area = vertical[next_index];
    let status_area = vertical[next_index + 1];

    if should_render_side_rail(state, main_area) {
        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(10),
                Constraint::Length(side_rail_width(main_area.width)),
            ])
            .split(main_area);
        render_main_pane(frame, horizontal[0], state);
        render_side_rail(frame, horizontal[1], state);
    } else {
        render_main_pane(frame, main_area, state);
    }
    if let Some(approval) = approval {
        render_approval_band(frame, approval_area.expect("approval area"), approval);
    }
    if let Some(command_hint) = command_hint.as_ref() {
        render_command_hint_band(
            frame,
            command_hint_area.expect("command hint area"),
            command_hint,
        );
    }
    render_composer(frame, composer_area, state);
    render_status_line(frame, status_area, state);

    let composer_inner = composer_inner_area(composer_area);
    let prefix_width = 2_u16;
    frame.set_cursor_position(Position::new(
        composer_inner
            .x
            .saturating_add(prefix_width)
            .saturating_add(composer_cursor_width(&state.input)),
        composer_inner.y,
    ));
}

fn composer_cursor_width(input: &str) -> u16 {
    UnicodeWidthStr::width(input).min(u16::MAX as usize) as u16
}

fn clamp_scroll(requested: u16, content_lines: usize, viewport_height: u16) -> u16 {
    let viewport = usize::from(viewport_height.max(1));
    let max_scroll = content_lines.saturating_sub(viewport);
    if requested == u16::MAX {
        max_scroll.min(u16::MAX as usize) as u16
    } else {
        usize::from(requested)
            .min(max_scroll)
            .min(u16::MAX as usize) as u16
    }
}

#[cfg(test)]
mod tests {
    use super::chrome::build_side_rail_lines;
    use super::statusline::format_footer_context;
    use super::transcript::build_transcript_lines;
    use super::view::{
        build_collection_text, build_command_palette_text, build_key_value_text,
        build_statusline_picker_text, should_render_view_title,
    };
    use super::welcome::build_welcome_lines;
    use super::{
        approval_preview_lines, build_approval_text, build_command_hint_text,
        should_render_side_rail,
    };
    use crate::frontend::tui::approval::ApprovalPrompt;
    use crate::frontend::tui::commands::{
        SlashCommandArgumentHint, SlashCommandArgumentSpec, SlashCommandArgumentValue,
        SlashCommandHint, SlashCommandSpec,
    };
    use crate::frontend::tui::state::{MainPaneMode, StatusLinePickerState, TodoEntry, TuiState};
    use ratatui::layout::Rect;

    #[test]
    fn key_value_text_renders_section_headers_without_treating_them_as_pairs() {
        let rendered = build_key_value_text(&[
            "## Session".to_string(),
            "session ref: abc123".to_string(),
            "/sessions [query]".to_string(),
        ]);
        let lines = rendered.lines;
        assert_eq!(lines[0].spans[0].content.as_ref(), "Session");
        assert_eq!(lines[1].spans[0].content.as_ref(), "session ref:");
        assert_eq!(lines[2].spans[0].content.as_ref(), "/sessions [query]");
    }

    #[test]
    fn key_value_text_preserves_prefixed_summary_blocks() {
        let rendered = build_key_value_text(&[
            "✔ Exported transcript text".to_string(),
            "  └ session-1".to_string(),
            "    Wrote 4 items to /workspace/out.txt".to_string(),
        ]);
        let lines = rendered.lines;
        assert_eq!(lines[0].spans[0].content.as_ref(), "✔");
        assert_eq!(
            lines[0].spans[2].content.as_ref(),
            "Exported transcript text"
        );
        assert_eq!(lines[1].spans[0].content.as_ref(), "  └ ");
        assert_eq!(
            lines[2].spans[1].content.as_ref(),
            "Wrote 4 items to /workspace/out.txt"
        );
    }

    #[test]
    fn key_value_text_reuses_transcript_rendering_for_shell_summary_lines() {
        let rendered = build_key_value_text(&[
            "• Reattached session".to_string(),
            "  └ session session-1".to_string(),
        ]);

        assert_eq!(rendered.lines[0].spans[0].content.as_ref(), "•");
        assert_eq!(
            rendered.lines[0].spans[2].content.as_ref(),
            "Reattached session"
        );
        assert_eq!(rendered.lines[1].spans[0].content.as_ref(), "  └ ");
        assert_eq!(
            rendered.lines[1].spans[1].content.as_ref(),
            "session session-1"
        );
    }

    #[test]
    fn transcript_entries_render_with_codex_like_prefixes() {
        let mut state = TuiState {
            main_pane: MainPaneMode::Transcript,
            ..TuiState::default()
        };
        state.transcript = vec!["• hello world".to_string()];

        let lines = build_transcript_lines(&state);

        assert_eq!(lines[0].spans[0].content.as_ref(), "•");
        assert_eq!(lines[0].spans[2].content.as_ref(), "hello world");
    }

    #[test]
    fn transcript_inserts_turn_dividers_between_user_turns() {
        let mut state = TuiState {
            main_pane: MainPaneMode::Transcript,
            ..TuiState::default()
        };
        state.transcript = vec![
            "› first".to_string(),
            "• reply".to_string(),
            "› second".to_string(),
        ];

        let rendered = build_transcript_lines(&state);

        assert!(rendered.iter().any(|line| {
            line.spans
                .first()
                .is_some_and(|span| span.content.contains("┈"))
        }));
    }

    #[test]
    fn transcript_separates_assistant_and_tool_entries_with_breathing_room() {
        let mut state = TuiState {
            main_pane: MainPaneMode::Transcript,
            ..TuiState::default()
        };
        state.transcript = vec![
            "• assistant reply".to_string(),
            "• Running bash\n  └ $ cargo test".to_string(),
            "› next prompt".to_string(),
        ];

        let rendered = build_transcript_lines(&state);

        assert_eq!(line_text_for(&rendered[0]), "• assistant reply");
        assert!(line_text_for(&rendered[1]).is_empty());
        assert_eq!(line_text_for(&rendered[2]), "• Running bash");
        assert!(
            rendered
                .iter()
                .any(|line| line_text_for(line).contains("hidden line"))
        );
        assert!(rendered.iter().any(|line| {
            line.spans
                .first()
                .is_some_and(|span| span.content.contains("┈"))
        }));
    }

    #[test]
    fn transcript_collapses_tool_details_by_default() {
        let mut state = TuiState {
            main_pane: MainPaneMode::Transcript,
            ..TuiState::default()
        };
        state.transcript = vec!["• Finished bash\n  └ exit 0\n```text\nok\n```".to_string()];

        let rendered = build_transcript_lines(&state);

        assert!(rendered.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("Finished bash"))
        }));
        assert!(rendered.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("hidden lines"))
        }));
        assert!(!rendered.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("exit 0"))
        }));
    }

    #[test]
    fn transcript_expands_tool_details_when_enabled() {
        let mut state = TuiState {
            main_pane: MainPaneMode::Transcript,
            show_tool_details: true,
            ..TuiState::default()
        };
        state.transcript = vec!["• Finished bash\n  └ exit 0\n```text\nok\n```".to_string()];

        let rendered = build_transcript_lines(&state);

        assert!(rendered.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("exit 0"))
        }));
        assert!(
            rendered
                .iter()
                .any(|line| line_text_for(line).contains("ok"))
        );
    }

    #[test]
    fn transcript_renders_resume_summary_above_history() {
        let mut state = TuiState {
            main_pane: MainPaneMode::Transcript,
            inspector_title: "Resume".to_string(),
            inspector: vec![
                "✔ Reattached session".to_string(),
                "  └ session session-1".to_string(),
            ],
            ..TuiState::default()
        };
        state.transcript = vec!["• done".to_string()];

        let rendered = build_transcript_lines(&state);

        assert_eq!(rendered[0].spans[0].content.as_ref(), "Resume");
        assert_eq!(rendered[2].spans[0].content.as_ref(), "✔");
        assert_eq!(rendered[2].spans[2].content.as_ref(), "Reattached session");
    }

    #[test]
    fn welcome_lines_keep_the_start_screen_sparse() {
        let mut state = TuiState::default();
        state.session.workspace_name = "nanoclaw".to_string();
        state.session.model = "gpt-5.4".to_string();

        let lines = build_welcome_lines(&state, 20);

        assert!(
            lines
                .iter()
                .any(|line| { line_text_for(line).contains("_   _    _    _   _  ___") })
        );
        assert!(
            lines
                .iter()
                .any(|line| { line_text_for(line).contains("nanoclaw · gpt-5.4") })
        );
        assert!(
            lines
                .iter()
                .any(|line| { line_text_for(line).contains("Type a prompt or /help.") })
        );
    }

    #[test]
    fn collection_text_renders_shell_summary_blocks_for_history_rows() {
        let rendered = build_collection_text(
            "Sessions",
            &[
                "## Sessions".to_string(),
                "• sess_123  no prompt yet\n  └ 12 messages · 40 events · 2 agent sessions · resume attached"
                    .to_string(),
            ],
        );

        assert_eq!(rendered.lines[1].spans[0].content.as_ref(), "›");
        assert_eq!(
            rendered.lines[1].spans[2].content.as_ref(),
            "sess_123  no prompt yet"
        );
        assert_eq!(rendered.lines[2].spans[0].content.as_ref(), "  ");
        assert_eq!(
            rendered.lines[2].spans[1].content.as_ref(),
            "12 messages · 40 events · 2 agent sessions · resume attached"
        );
    }

    #[test]
    fn collection_text_keeps_history_rows_compact() {
        let rendered = build_collection_text(
            "Sessions",
            &[
                "• sess_123  no prompt yet\n  └ 12 messages · 40 events".to_string(),
                "• sess_456  resume prompt\n  └ 4 messages · 9 events".to_string(),
            ],
        );

        assert_eq!(rendered.lines[2].spans[0].content.as_ref(), "›");
        assert_eq!(
            rendered.lines[2].spans[2].content.as_ref(),
            "sess_456  resume prompt"
        );
    }

    #[test]
    fn statusline_picker_text_renders_checked_rows() {
        let rendered = build_statusline_picker_text(
            &TuiState::default().session.statusline,
            &StatusLinePickerState { selected: 1 },
        );

        assert_eq!(rendered.lines[0].spans[0].content.as_ref(), "status line");
        assert!(
            rendered
                .lines
                .iter()
                .any(|line| line_text_for(line).contains("› [x] model"))
        );
        assert!(
            rendered
                .lines
                .iter()
                .any(|line| line_text_for(line).contains("[ ] session"))
        );
    }

    #[test]
    fn command_palette_text_matches_picker_style() {
        let rendered = build_command_palette_text(&[
            "## Session".to_string(),
            "/help [query]  browse commands".to_string(),
            "/sessions [query]  browse persisted sessions".to_string(),
        ]);

        assert_eq!(rendered.lines[0].spans[0].content.as_ref(), "Session");
        assert_eq!(rendered.lines[1].spans[0].content.as_ref(), "›");
        assert_eq!(rendered.lines[1].spans[2].content.as_ref(), "/help [query]");
        assert_eq!(
            rendered.lines[1].spans[4].content.as_ref(),
            "browse commands"
        );
        assert_eq!(
            rendered.lines[2].spans[2].content.as_ref(),
            "/sessions [query]"
        );
    }

    #[test]
    fn transcript_renders_compact_live_progress_line() {
        let state = TuiState {
            main_pane: MainPaneMode::Transcript,
            turn_running: true,
            status: "Working (2)".to_string(),
            ..TuiState::default()
        };

        let rendered = build_transcript_lines(&state);

        assert!(rendered.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("Working (2)"))
        }));
        assert!(!rendered.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("$ cargo test"))
        }));
    }

    #[test]
    fn transcript_hides_progress_line_while_tool_cell_is_active() {
        let state = TuiState {
            main_pane: MainPaneMode::Transcript,
            turn_running: true,
            status: "Working".to_string(),
            active_tool_label: Some("bash".to_string()),
            transcript: vec!["• Running bash\n  └ $ cargo test".to_string()],
            ..TuiState::default()
        };

        let rendered = build_transcript_lines(&state);

        let running_count = rendered
            .iter()
            .filter(|line| {
                line.spans
                    .iter()
                    .any(|span| span.content.as_ref().contains("Running bash"))
            })
            .count();
        assert_eq!(running_count, 1);
    }

    #[test]
    fn transcript_renders_markdown_blocks_without_fence_noise() {
        let mut state = TuiState {
            main_pane: MainPaneMode::Transcript,
            ..TuiState::default()
        };
        state.transcript = vec![
            concat!(
                "• # Plan\n",
                "- inspect output\n",
                "1. rerun tests\n",
                "> keep the diff readable\n",
                "Use `rg` for search\n",
                "```diff\n",
                "+ added line\n",
                "- removed line\n",
                "@@ hunk\n",
                "```"
            )
            .to_string(),
        ];

        let rendered = build_transcript_lines(&state);
        assert_eq!(rendered[0].spans[0].content.as_ref(), "•");
        assert_eq!(rendered[0].spans[2].content.as_ref(), "Plan");
        assert!(rendered.iter().all(|line| {
            line.spans
                .iter()
                .all(|span| !span.content.as_ref().contains("```"))
        }));
        assert!(
            rendered
                .iter()
                .any(|line| line_text_for(line).contains("inspect output"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| line_text_for(line).contains("rerun tests"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| line_text_for(line).contains("keep the diff readable"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| { line.spans.iter().any(|span| span.content.as_ref() == "rg") })
        );
        assert!(
            rendered
                .iter()
                .any(|line| line_text_for(line).contains("+ added line"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| line_text_for(line).contains("- removed line"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| line_text_for(line).contains("@@ hunk"))
        );
        assert!(
            rendered
                .iter()
                .any(|line| line_text_for(line).contains("··· diff"))
        );
    }

    #[test]
    fn transcript_keeps_fenced_block_label_as_first_visible_line() {
        let mut state = TuiState {
            main_pane: MainPaneMode::Transcript,
            ..TuiState::default()
        };
        state.transcript = vec!["• ```rust\nfn main() {}\n```".to_string()];

        let rendered = build_transcript_lines(&state);

        let first_visible = rendered
            .iter()
            .find(|line| !line_text_for(line).trim().is_empty())
            .expect("expected visible transcript line");
        assert_eq!(line_text_for(first_visible), "• ··· rust");
        assert!(
            rendered
                .iter()
                .any(|line| line_text_for(line).contains("fn main() {}"))
        );
    }

    fn line_text_for(line: &ratatui::text::Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn side_rail_surfaces_todos_and_lsp_summary() {
        let mut state = TuiState::default();
        state.main_pane = MainPaneMode::Transcript;
        state.session.tool_names = vec!["code_symbol_search".to_string()];
        state.session.startup_diagnostics.diagnostics = vec!["rust-analyzer attached".to_string()];
        state.todo_items = vec![
            TodoEntry {
                id: "t1".to_string(),
                content: "Refine transcript".to_string(),
                status: "in_progress".to_string(),
            },
            TodoEntry {
                id: "t2".to_string(),
                content: "Tighten command palette".to_string(),
                status: "pending".to_string(),
            },
            TodoEntry {
                id: "t3".to_string(),
                content: "Finish diagnostics".to_string(),
                status: "completed".to_string(),
            },
        ];

        let lines = build_side_rail_lines(&state);

        assert!(line_text_for(&lines[0]).contains("LSP"));
        assert!(lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("0 warnings · 1 diagnostics"))
        }));
        assert!(lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("rust-analyzer attached"))
        }));
        assert!(lines.iter().any(|line| {
            line.spans.iter().any(|span| {
                span.content
                    .as_ref()
                    .contains("1 active · 1 pending · 1 done")
            })
        }));
        assert!(lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("Refine transcript"))
        }));
    }

    #[test]
    fn side_rail_stays_hidden_for_non_transcript_views() {
        let mut state = TuiState::default();
        state.main_pane = MainPaneMode::View;
        state.session.tool_names = vec!["code_symbol_search".to_string()];

        assert!(!should_render_side_rail(
            &state,
            Rect {
                x: 0,
                y: 0,
                width: 140,
                height: 20,
            }
        ));
    }

    #[test]
    fn approval_band_uses_structured_command_preview() {
        let text = build_approval_text(&ApprovalPrompt {
            tool_name: "bash".to_string(),
            origin: "local".to_string(),
            mode: Some("run".to_string()),
            working_directory: Some("/workspace/apps/code-agent".to_string()),
            content_label: "command".to_string(),
            content_preview: vec!["$ cargo test".to_string()],
            reasons: vec!["sandbox policy requires approval".to_string()],
        });

        assert!(line_text_for(&text.lines[0]).contains("Approve bash?"));
        assert!(
            text.lines[1]
                .spans
                .iter()
                .any(|span| { span.content.as_ref().contains("/workspace/apps/code-agent") })
        );
        assert_eq!(text.lines[2].spans[0].content.as_ref(), "command");
        assert_eq!(text.lines[4].spans[0].content.as_ref(), "why");
        assert!(text.lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.as_ref().contains("$ cargo test"))
        }));
        assert!(text.lines.iter().any(|line| {
            line.spans.iter().any(|span| {
                span.content
                    .as_ref()
                    .contains("sandbox policy requires approval")
            })
        }));
    }

    #[test]
    fn approval_preview_lines_collapse_long_argument_blocks() {
        let lines = approval_preview_lines(&[
            "one".to_string(),
            "two".to_string(),
            "three".to_string(),
            "four".to_string(),
            "five".to_string(),
        ]);

        assert_eq!(lines, vec!["one", "two", "...", "five"]);
    }

    #[test]
    fn command_hint_text_surfaces_selected_usage_and_matches() {
        let rendered = build_command_hint_text(&SlashCommandHint {
            selected: SlashCommandSpec {
                section: "History",
                name: "sessions",
                usage: "sessions [query]",
                summary: "browse persisted sessions",
            },
            matches: vec![
                SlashCommandSpec {
                    section: "History",
                    name: "sessions",
                    usage: "sessions [query]",
                    summary: "browse persisted sessions",
                },
                SlashCommandSpec {
                    section: "History",
                    name: "session",
                    usage: "session <session-ref>",
                    summary: "open persisted session",
                },
            ],
            selected_match_index: 0,
            arguments: None,
            exact: false,
        });

        assert_eq!(rendered.lines[0].spans[0].content.as_ref(), "commands");
        assert_eq!(rendered.lines[1].spans[0].content.as_ref(), "›");
        assert_eq!(
            rendered.lines[1].spans[2].content.as_ref(),
            "/sessions [query]"
        );
        assert_eq!(
            rendered.lines[1].spans[4].content.as_ref(),
            "browse persisted sessions"
        );
        assert_eq!(
            rendered.lines[2].spans[1].content.as_ref(),
            "/session <session-ref>"
        );
        assert_eq!(rendered.lines[3].spans[3].content.as_ref(), "tab complete");
        assert_eq!(rendered.lines[3].spans[7].content.as_ref(), "enter accept");
    }

    #[test]
    fn command_hint_text_surfaces_argument_progress() {
        let rendered = build_command_hint_text(&SlashCommandHint {
            selected: SlashCommandSpec {
                section: "Agents",
                name: "spawn_task",
                usage: "spawn_task <role> <prompt>",
                summary: "launch child agent",
            },
            matches: vec![SlashCommandSpec {
                section: "Agents",
                name: "spawn_task",
                usage: "spawn_task <role> <prompt>",
                summary: "launch child agent",
            }],
            selected_match_index: 0,
            arguments: Some(SlashCommandArgumentHint {
                provided: vec![SlashCommandArgumentValue {
                    placeholder: "<role>",
                    value: "reviewer".to_string(),
                }],
                next: Some(SlashCommandArgumentSpec {
                    placeholder: "<prompt>",
                    required: true,
                }),
            }),
            exact: true,
        });

        assert_eq!(rendered.lines[2].spans[1].content.as_ref(), "<role>");
        assert_eq!(rendered.lines[2].spans[3].content.as_ref(), "reviewer");
        assert!(
            rendered.lines[2]
                .spans
                .iter()
                .any(|span| span.content.as_ref().contains("<prompt>"))
        );
        assert_eq!(rendered.lines[3].spans[3].content.as_ref(), "keep typing");
        assert_eq!(rendered.lines[3].spans[7].content.as_ref(), "keep typing");
    }

    #[test]
    fn command_hint_text_keeps_enter_run_for_optional_arguments() {
        let rendered = build_command_hint_text(&SlashCommandHint {
            selected: SlashCommandSpec {
                section: "Session",
                name: "help",
                usage: "help [query]",
                summary: "browse commands",
            },
            matches: vec![SlashCommandSpec {
                section: "Session",
                name: "help",
                usage: "help [query]",
                summary: "browse commands",
            }],
            selected_match_index: 0,
            arguments: Some(SlashCommandArgumentHint {
                provided: Vec::new(),
                next: Some(SlashCommandArgumentSpec {
                    placeholder: "[query]",
                    required: false,
                }),
            }),
            exact: true,
        });

        assert_eq!(rendered.lines[2].spans[1].content.as_ref(), "[query]");
        assert_eq!(rendered.lines[3].spans[3].content.as_ref(), "enter run");
        assert_eq!(rendered.lines[3].spans[7].content.as_ref(), "enter run");
    }

    #[test]
    fn command_hint_text_shows_browse_window_ellipsis() {
        let rendered = build_command_hint_text(&SlashCommandHint {
            selected: SlashCommandSpec {
                section: "History",
                name: "resume",
                usage: "resume <agent-session-ref>",
                summary: "reattach agent session",
            },
            matches: vec![
                SlashCommandSpec {
                    section: "Session",
                    name: "help",
                    usage: "help",
                    summary: "browse commands",
                },
                SlashCommandSpec {
                    section: "Session",
                    name: "status",
                    usage: "status",
                    summary: "session overview",
                },
                SlashCommandSpec {
                    section: "Session",
                    name: "new",
                    usage: "new",
                    summary: "fresh top-level session",
                },
                SlashCommandSpec {
                    section: "History",
                    name: "sessions",
                    usage: "sessions [query]",
                    summary: "browse persisted sessions",
                },
                SlashCommandSpec {
                    section: "History",
                    name: "session",
                    usage: "session <session-ref>",
                    summary: "open persisted session",
                },
                SlashCommandSpec {
                    section: "History",
                    name: "resume",
                    usage: "resume <agent-session-ref>",
                    summary: "reattach agent session",
                },
                SlashCommandSpec {
                    section: "Agents",
                    name: "live_tasks",
                    usage: "live_tasks",
                    summary: "list live child agents",
                },
            ],
            selected_match_index: 5,
            arguments: None,
            exact: false,
        });

        assert_eq!(rendered.lines[0].spans[0].content.as_ref(), "commands");
        assert_eq!(rendered.lines[1].spans[0].content.as_ref(), "… 2 earlier");
        assert_eq!(
            rendered.lines[5].spans[2].content.as_ref(),
            "/resume <agent-session-ref>"
        );
        assert_eq!(rendered.lines[6].spans[0].content.as_ref(), "… 1 more");
    }

    #[test]
    fn footer_context_renders_configured_status_items() {
        let mut state = TuiState::default();
        state.status = "Ready".to_string();
        state.session.workspace_name = "nanoclaw".to_string();
        state.session.model = "gpt-5.4".to_string();
        state.session.model_reasoning_effort = Some("high".to_string());
        state.session.active_session_ref = "session_123456".to_string();
        state.session.git.available = true;
        state.session.git.repo_name = "nanoclaw-repo".to_string();
        state.session.git.branch = "main".to_string();
        state.session.statusline.clock = false;
        state.session.statusline.session = true;

        let footer = format_footer_context(&state);

        assert_eq!(footer.spans[0].content.as_ref(), "•");
        assert!(
            footer
                .spans
                .iter()
                .any(|span| { span.content.as_ref().contains("Ready") })
        );
        assert!(
            footer
                .spans
                .iter()
                .any(|span| { span.content.as_ref().contains("nanoclaw") })
        );
        assert!(
            footer
                .spans
                .iter()
                .any(|span| { span.content.as_ref().contains("gpt-5.4 (high)") })
        );
        assert!(
            footer
                .spans
                .iter()
                .any(|span| { span.content.as_ref().contains("nanoclaw-repo") })
        );
        assert!(
            footer
                .spans
                .iter()
                .any(|span| { span.content.as_ref().contains("main") })
        );
        assert!(
            footer
                .spans
                .iter()
                .any(|span| { span.content.as_ref().contains("ctx --") })
        );
        assert!(
            footer
                .spans
                .iter()
                .any(|span| { span.content.as_ref().contains("in 0") })
        );
        assert!(
            footer
                .spans
                .iter()
                .any(|span| { span.content.as_ref().contains("out 0") })
        );
        assert!(
            footer
                .spans
                .iter()
                .any(|span| { span.content.as_ref().contains("queue 0") })
        );
        assert!(
            footer
                .spans
                .iter()
                .any(|span| { span.content.as_ref().contains("session_") })
        );
    }

    #[test]
    fn footer_context_window_includes_units_and_percent() {
        let mut state = TuiState::default();
        state.session.token_ledger.context_window = Some(agent::types::ContextWindowUsage {
            used_tokens: 30_000,
            max_tokens: 400_000,
        });

        let footer = format_footer_context(&state);

        assert!(
            footer
                .spans
                .iter()
                .any(|span| span.content.as_ref().contains("ctx 30k / 400k tok (7%)"))
        );
    }

    #[test]
    fn composer_cursor_width_accounts_for_wide_characters() {
        assert_eq!(super::composer_cursor_width("hello"), 5);
        assert_eq!(super::composer_cursor_width("你好"), 4);
        assert_eq!(super::composer_cursor_width("A你B"), 4);
    }

    #[test]
    fn view_title_is_suppressed_when_the_collection_already_has_one() {
        assert!(!should_render_view_title(
            "Sessions",
            &["## Sessions".to_string(), "• sess_123  prompt".to_string()]
        ));
        assert!(should_render_view_title(
            "Export",
            &["## Session".to_string(), "path: out.txt".to_string()]
        ));
    }
}
