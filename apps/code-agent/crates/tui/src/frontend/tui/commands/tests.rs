use super::{
    SlashCommand, SlashCommandArgumentSpec, SlashCommandEnterAction, command_palette_lines,
    command_palette_lines_for, cycle_slash_command, inspector_action_for_slash_name,
    move_slash_command_selection, parse_slash_command, resolve_slash_enter_action,
    slash_command_hint,
};
use crate::frontend::tui::state::{InspectorAction, InspectorEntry};

#[test]
fn parses_session_query_with_spaces() {
    match parse_slash_command("/sessions fix failing test") {
        SlashCommand::Sessions { query } => {
            assert_eq!(query, Some("fix failing test".to_string()));
        }
        _ => panic!("unexpected command"),
    }
}

#[test]
fn parses_btw_question_with_spaces() {
    match parse_slash_command("/btw what changed in the deploy flow") {
        SlashCommand::Btw { question } => {
            assert_eq!(
                question,
                Some("what changed in the deploy flow".to_string())
            );
        }
        _ => panic!("unexpected command"),
    }
}

#[test]
fn export_transcript_keeps_path_tail_intact() {
    match parse_slash_command("/export_transcript abc123 reports/run log.txt") {
        SlashCommand::ExportTranscript { session_ref, path } => {
            assert_eq!(session_ref, "abc123");
            assert_eq!(path, "reports/run log.txt");
        }
        _ => panic!("unexpected command"),
    }
}

#[test]
fn parses_agent_session_listing_with_optional_session_ref() {
    match parse_slash_command("/agent_sessions abc123") {
        SlashCommand::AgentSessions { session_ref } => {
            assert_eq!(session_ref, Some("abc123".to_string()));
        }
        _ => panic!("unexpected command"),
    }
}

#[test]
fn parses_agent_session_lookup() {
    match parse_slash_command("/agent_session agent123") {
        SlashCommand::AgentSession { agent_session_ref } => {
            assert_eq!(agent_session_ref, "agent123");
        }
        _ => panic!("unexpected command"),
    }
}

#[test]
fn parses_task_listing_with_optional_session_ref() {
    match parse_slash_command("/tasks abc123") {
        SlashCommand::Tasks { session_ref } => {
            assert_eq!(session_ref, Some("abc123".to_string()));
        }
        _ => panic!("unexpected command"),
    }
}

#[test]
fn parses_task_lookup() {
    match parse_slash_command("/task review-task") {
        SlashCommand::Task { task_ref } => {
            assert_eq!(task_ref, "review-task");
        }
        _ => panic!("unexpected command"),
    }
}

#[test]
fn command_palette_marks_required_argument_commands_as_input_seeds() {
    assert_eq!(
        inspector_action_for_slash_name("session"),
        Some(InspectorAction::FillInput("/session ".to_string()))
    );
    assert_eq!(
        inspector_action_for_slash_name("details"),
        Some(InspectorAction::RunCommand("/details".to_string()))
    );
}

#[test]
fn parses_live_task_listing() {
    assert!(matches!(
        parse_slash_command("/live_tasks"),
        SlashCommand::LiveTasks
    ));
}

#[test]
fn parses_permissions_mode_switch() {
    match parse_slash_command("/permissions danger-full-access") {
        SlashCommand::Permissions { mode } => {
            assert_eq!(
                mode,
                Some(crate::interaction::SessionPermissionMode::DangerFullAccess)
            );
        }
        _ => panic!("unexpected command"),
    }
}

#[test]
fn parses_spawn_task_with_prompt_tail() {
    match parse_slash_command("/spawn_task reviewer inspect failing tests") {
        SlashCommand::SpawnTask { role, prompt } => {
            assert_eq!(role, "reviewer");
            assert_eq!(prompt, "inspect failing tests");
        }
        _ => panic!("unexpected command"),
    }
}

#[test]
fn spawn_task_requires_prompt() {
    match parse_slash_command("/spawn_task reviewer") {
        SlashCommand::InvalidUsage(message) => {
            assert!(message.contains("Usage:"));
            assert!(message.contains("spawn_task <ROLE> <PROMPT>"));
        }
        _ => panic!("unexpected command"),
    }
}

#[test]
fn parses_send_task_with_message_tail() {
    match parse_slash_command("/send_task review-task focus on tests") {
        SlashCommand::SendTask {
            task_or_agent_ref,
            message,
        } => {
            assert_eq!(task_or_agent_ref, "review-task");
            assert_eq!(message, Some("focus on tests".to_string()));
        }
        _ => panic!("unexpected command"),
    }
}

#[test]
fn parses_wait_task_lookup() {
    match parse_slash_command("/wait_task review-task") {
        SlashCommand::WaitTask { task_or_agent_ref } => {
            assert_eq!(task_or_agent_ref, "review-task");
        }
        _ => panic!("unexpected command"),
    }
}

#[test]
fn parses_cancel_task_with_optional_reason_tail() {
    match parse_slash_command("/cancel_task review-task fix it now") {
        SlashCommand::CancelTask {
            task_or_agent_ref,
            reason,
        } => {
            assert_eq!(task_or_agent_ref, "review-task");
            assert_eq!(reason, Some("fix it now".to_string()));
        }
        _ => panic!("unexpected command"),
    }
}

#[test]
fn missing_session_ref_returns_usage_error() {
    match parse_slash_command("/session") {
        SlashCommand::InvalidUsage(message) => {
            assert!(message.contains("Usage:"));
            assert!(message.contains("session <SESSION_REF>"));
        }
        _ => panic!("unexpected command"),
    }
}

#[test]
fn parses_mcp_prompt_lookup() {
    match parse_slash_command("/prompt fs code_review") {
        SlashCommand::Prompt {
            server_name,
            prompt_name,
        } => {
            assert_eq!(server_name, "fs");
            assert_eq!(prompt_name, "code_review");
        }
        _ => panic!("unexpected command"),
    }
}

#[test]
fn parses_new_and_clear_as_same_session_operation() {
    assert!(matches!(parse_slash_command("/new"), SlashCommand::New));
    assert!(matches!(parse_slash_command("/clear"), SlashCommand::New));
}

#[test]
fn parses_details_toggle() {
    assert!(matches!(
        parse_slash_command("/details"),
        SlashCommand::Details
    ));
}

#[test]
fn parses_statusline_picker_command() {
    assert!(matches!(
        parse_slash_command("/statusline"),
        SlashCommand::StatusLine
    ));
}

#[test]
fn parses_queue_command() {
    assert!(matches!(parse_slash_command("/queue"), SlashCommand::Queue));
}

#[test]
fn parses_thinking_effort_command() {
    assert_eq!(
        parse_slash_command("/thinking high"),
        SlashCommand::Thinking {
            effort: Some("high".to_string())
        }
    );
    assert_eq!(
        parse_slash_command("/thinking"),
        SlashCommand::Thinking { effort: None }
    );
}

#[test]
fn parses_theme_command() {
    assert_eq!(
        parse_slash_command("/theme fjord"),
        SlashCommand::Theme {
            name: Some("fjord".to_string())
        }
    );
    assert_eq!(
        parse_slash_command("/theme"),
        SlashCommand::Theme { name: None }
    );
}

#[test]
fn parses_image_and_file_attachment_commands() {
    assert_eq!(
        parse_slash_command("/image artifacts/failure.png"),
        SlashCommand::Image {
            path: "artifacts/failure.png".to_string()
        }
    );
    assert_eq!(
        parse_slash_command("/file reports/run log.pdf"),
        SlashCommand::File {
            path: "reports/run log.pdf".to_string()
        }
    );
}

#[test]
fn parses_detach_with_optional_index() {
    assert_eq!(
        parse_slash_command("/detach 2"),
        SlashCommand::Detach { index: Some(2) }
    );
    assert_eq!(
        parse_slash_command("/detach"),
        SlashCommand::Detach { index: None }
    );
}

#[test]
fn parses_move_attachment_command() {
    assert_eq!(
        parse_slash_command("/move_attachment 2 1"),
        SlashCommand::MoveAttachment { from: 2, to: 1 }
    );
}

#[test]
fn command_palette_includes_help_and_clear_alias() {
    let lines = inspector_line_texts(&command_palette_lines());

    assert!(lines.iter().any(|line| line == "## Session"));
    assert!(
        lines
            .iter()
            .any(|line| line == "/help [query]  browse commands")
    );
    assert!(
        lines
            .iter()
            .any(|line| line == "/details  toggle tool details")
    );
    assert!(
        lines
            .iter()
            .any(|line| line == "/statusline  toggle footer items")
    );
    assert!(
        lines
            .iter()
            .any(|line| line == "/thinking [level]  pick or set thinking effort")
    );
    assert!(
        lines
            .iter()
            .any(|line| line == "/theme [name]  pick or set the tui theme")
    );
    assert!(lines.iter().any(|line| line == "/clear  alias of /new"));
    assert!(
        lines
            .iter()
            .any(|line| { line == "/exit  leave tui · aliases: /quit /q" })
    );
}

#[test]
fn command_palette_can_filter_by_query() {
    let lines = inspector_line_texts(&command_palette_lines_for(Some("agent")));

    assert!(lines.iter().any(|line| line == "## Agents"));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("/agent_sessions [session-ref]"))
    );
    assert!(!lines.iter().any(|line| line.contains("/export_transcript")));
}

fn inspector_line_texts(lines: &[InspectorEntry]) -> Vec<String> {
    lines
        .iter()
        .map(|line| match line {
            InspectorEntry::Section(text)
            | InspectorEntry::Plain(text)
            | InspectorEntry::Muted(text)
            | InspectorEntry::Command(text) => {
                if matches!(line, InspectorEntry::Section(_)) {
                    format!("## {text}")
                } else {
                    text.clone()
                }
            }
            InspectorEntry::Field { key, value } => format!("{key}: {value}"),
            InspectorEntry::Transcript(entry) => entry.serialized(),
            InspectorEntry::CollectionItem {
                primary, secondary, ..
            } => secondary
                .as_ref()
                .map(|secondary| format!("{primary}  {secondary}"))
                .unwrap_or_else(|| primary.clone()),
            InspectorEntry::Empty => String::new(),
        })
        .collect()
}

#[test]
fn parses_help_query_tail() {
    match parse_slash_command("/help agent") {
        SlashCommand::Help { query } => {
            assert_eq!(query, Some("agent".to_string()));
        }
        _ => panic!("unexpected command"),
    }
}

#[test]
fn slash_command_hint_matches_prefix() {
    let hint = slash_command_hint("/sess", 0).expect("hint");

    assert_eq!(hint.selected.name, "sessions");
    assert_eq!(hint.selected_match_index, 0);
    assert!(hint.matches.iter().any(|spec| spec.name == "sessions"));
    assert!(hint.matches.iter().any(|spec| spec.name == "session"));
    assert!(hint.arguments.is_none());
}

#[test]
fn slash_command_hint_matches_exit_alias_prefix() {
    let hint = slash_command_hint("/q", 0).expect("hint");

    assert_eq!(hint.selected.name, "exit");
    assert!(hint.exact);
    assert!(hint.matches.iter().any(|spec| spec.name == "exit"));
}

#[test]
fn cycle_slash_command_completes_partial_input() {
    let (input, index) = cycle_slash_command("/sess", 0, false).expect("completion");

    assert_eq!(input, "/sessions ");
    assert_eq!(index, 0);
}

#[test]
fn cycle_slash_command_cycles_backward() {
    let (input, index) = cycle_slash_command("/sess", 0, true).expect("completion");

    assert_eq!(input, "/session ");
    assert_eq!(index, 1);
}

#[test]
fn cycle_slash_command_stops_after_args_begin() {
    assert!(cycle_slash_command("/session abc123", 0, false).is_none());
}

#[test]
fn move_slash_command_selection_keeps_partial_input_in_picker() {
    let next = move_slash_command_selection("/sess", 0, false).expect("selection");

    assert_eq!(next, 1);
}

#[test]
fn slash_command_hint_surfaces_next_required_argument() {
    let hint = slash_command_hint("/session ", 0).expect("hint");

    let arguments = hint.arguments.expect("arguments");
    assert_eq!(
        arguments.next,
        Some(SlashCommandArgumentSpec {
            placeholder: "<session-ref>",
            required: true,
        })
    );
    assert!(arguments.provided.is_empty());
    assert_eq!(hint.selected_match_index, 0);
}

#[test]
fn slash_command_hint_tracks_argument_progress() {
    let hint = slash_command_hint("/spawn_task reviewer", 0).expect("hint");

    let arguments = hint.arguments.expect("arguments");
    assert_eq!(arguments.provided.len(), 1);
    assert_eq!(arguments.provided[0].placeholder, "<role>");
    assert_eq!(arguments.provided[0].value, "reviewer");
    assert_eq!(
        arguments.next,
        Some(SlashCommandArgumentSpec {
            placeholder: "<prompt>",
            required: true,
        })
    );
}

#[test]
fn slash_command_hint_browses_all_commands_from_empty_slash() {
    let hint = slash_command_hint("/", 0).expect("hint");

    assert_eq!(hint.selected.name, "help");
    assert_eq!(hint.selected_match_index, 0);
    assert!(hint.matches.len() > 10);
    assert!(hint.matches.iter().any(|spec| spec.name == "spawn_task"));
}

#[test]
fn slash_enter_action_completes_ambiguous_partial_command() {
    let action = resolve_slash_enter_action("/sess", 0).expect("action");

    assert_eq!(
        action,
        SlashCommandEnterAction::Complete {
            input: "/sessions ".to_string(),
            index: 0,
        }
    );
}

#[test]
fn slash_enter_action_executes_unique_no_arg_command() {
    let action = resolve_slash_enter_action("/he", 0).expect("action");

    assert_eq!(
        action,
        SlashCommandEnterAction::Execute("/help".to_string())
    );
}

#[test]
fn exact_required_command_is_prioritized_in_hint() {
    let hint = slash_command_hint("/session", 0).expect("hint");

    assert_eq!(hint.selected.name, "session");
    assert!(hint.exact);
}

#[test]
fn slash_enter_action_accepts_required_argument_command_before_running() {
    let action = resolve_slash_enter_action("/session", 0).expect("action");

    assert_eq!(
        action,
        SlashCommandEnterAction::Complete {
            input: "/session ".to_string(),
            index: 0,
        }
    );
}
