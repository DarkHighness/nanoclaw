use super::*;

pub(super) fn plain_input_submit_action(
    input: &str,
    has_prompt_content: bool,
    requires_prompt_submission: bool,
    turn_running: bool,
    key: KeyCode,
) -> Option<PlainInputSubmitAction> {
    if !has_prompt_content || input.starts_with('/') {
        return None;
    }
    match (turn_running, key) {
        (true, KeyCode::Enter) if requires_prompt_submission => {
            Some(PlainInputSubmitAction::QueuePrompt)
        }
        (true, KeyCode::Enter) => Some(PlainInputSubmitAction::SteerActiveTurn),
        (true, KeyCode::Tab) => Some(PlainInputSubmitAction::QueuePrompt),
        (false, KeyCode::Enter) => Some(PlainInputSubmitAction::StartPrompt),
        _ => None,
    }
}

pub(super) fn merge_interrupt_steers(steers: Vec<String>) -> Option<String> {
    if steers.is_empty() {
        None
    } else {
        Some(steers.join("\n"))
    }
}

pub(super) fn build_history_rollback_candidates(
    rounds: &[HistoryRollbackRound],
) -> Vec<state::HistoryRollbackCandidate> {
    rounds
        .iter()
        .map(|round| {
            let prompt = agent::types::message_operator_text(&round.prompt_message);
            let draft = state::composer_draft_from_message(&round.prompt_message);
            state::HistoryRollbackCandidate {
                message_id: round.rollback_message_id.clone(),
                prompt,
                draft,
                turn_preview_lines: format_visible_transcript_preview_lines(&round.round_messages),
                removed_turn_count: round.removed_turn_count,
                removed_message_count: round.removed_message_count,
            }
        })
        .collect()
}

pub(super) fn history_rollback_status(
    candidate: &state::HistoryRollbackCandidate,
    selected: usize,
    total: usize,
) -> String {
    format!(
        "Rollback turn {} of {} · removes {} turn(s) / {} message(s) · {}",
        selected + 1,
        total,
        candidate.removed_turn_count,
        candidate.removed_message_count,
        state::draft_preview_text(&candidate.draft, &candidate.prompt, 40)
    )
}

pub(super) fn attachment_preview_status_label(
    preview: &state::ComposerRowAttachmentPreview,
) -> String {
    format!("attachment #{} · {}", preview.index, preview.summary)
}

pub(super) fn removed_attachment_status_label(
    preview: Option<&state::ComposerRowAttachmentPreview>,
    attachment: &ComposerDraftAttachmentState,
) -> String {
    preview
        .map(attachment_preview_status_label)
        .or_else(|| {
            attachment
                .row_summary()
                .map(|summary| format!("attachment · {summary}"))
        })
        .unwrap_or_else(|| "attachment".to_string())
}

pub(super) fn external_editor_attachment_status_suffix(
    summary: &state::ComposerAttachmentEditSummary,
) -> String {
    external_editor_attachment_feedback_suffix(summary)
}

pub(super) fn external_editor_attachment_activity_suffix(
    summary: &state::ComposerAttachmentEditSummary,
) -> String {
    external_editor_attachment_feedback_suffix(summary)
}

pub(super) fn external_editor_attachment_feedback_suffix(
    summary: &state::ComposerAttachmentEditSummary,
) -> String {
    match (summary.detached.len(), summary.reordered) {
        (0, false) => String::new(),
        (0, true) => " · reordered attachments".to_string(),
        (1, false) => format!(
            " · detached {}",
            attachment_preview_status_label(&summary.detached[0])
        ),
        (count, false) => format!(" · detached {count} attachments"),
        (1, true) => format!(
            " · detached {} and reordered remaining",
            attachment_preview_status_label(&summary.detached[0])
        ),
        (count, true) => format!(" · detached {count} attachments and reordered remaining"),
    }
}

pub(super) fn preview_path_tail(path: &str) -> String {
    if let Some(segment) = remote_attachment_tail_segment(path) {
        return segment;
    }
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(path)
        .to_string()
}

pub(super) fn looks_like_local_image_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg")
    )
}

pub(super) fn is_remote_attachment_url(path: &str) -> bool {
    matches!(path.trim(), value if value.starts_with("http://") || value.starts_with("https://"))
}

pub(super) fn remote_attachment_tail_segment(path: &str) -> Option<String> {
    let (_, remainder) = path.trim().split_once("://")?;
    let path = remainder
        .split_once('/')
        .map(|(_, path)| path)
        .unwrap_or_default();
    let trimmed = path
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim_matches('/');
    (!trimmed.is_empty()).then(|| {
        trimmed
            .rsplit('/')
            .find(|segment| !segment.is_empty())
            .unwrap_or(trimmed)
            .to_string()
    })
}

pub(super) fn remote_attachment_file_name(path: &str) -> Option<String> {
    remote_attachment_tail_segment(path).filter(|segment| !segment.is_empty())
}

pub(super) async fn load_composer_file(
    requested_path: &str,
    ctx: &ToolExecutionContext,
) -> Result<LoadedComposerFile> {
    let resolved_path = resolve_tool_path_against_workspace_root(
        requested_path,
        ctx.effective_root(),
        ctx.container_workdir.as_deref(),
    )?;
    ctx.assert_path_read_allowed(&resolved_path)?;
    let bytes = fs::read(&resolved_path).await?;
    Ok(LoadedComposerFile {
        requested_path: requested_path.to_string(),
        file_name: resolved_path
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_string),
        mime_type: sniff_composer_file_mime(&bytes, &resolved_path).map(str::to_string),
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

pub(super) fn sniff_composer_file_mime(bytes: &[u8], path: &Path) -> Option<&'static str> {
    if bytes.starts_with(b"%PDF-") {
        return Some("application/pdf");
    }
    match path.extension().and_then(|value| value.to_str()) {
        Some("pdf") => Some("application/pdf"),
        _ => None,
    }
}

pub(super) fn sniff_remote_image_mime(path: &str) -> Option<&'static str> {
    match remote_attachment_extension(path)?.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

pub(super) fn sniff_remote_file_mime(path: &str) -> Option<&'static str> {
    match remote_attachment_extension(path)?.as_str() {
        "pdf" => Some("application/pdf"),
        _ => None,
    }
}

pub(super) fn remote_attachment_extension(path: &str) -> Option<String> {
    let segment = remote_attachment_tail_segment(path)?;
    segment
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .and_then(|extension| {
            let normalized = extension.trim();
            (!normalized.is_empty()).then_some(normalized.to_ascii_lowercase())
        })
}

pub(super) fn resolve_external_editor_command() -> Result<Vec<String>> {
    let configured = env::var("VISUAL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("EDITOR")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .ok_or_else(|| anyhow!("Cannot open external editor: set $VISUAL or $EDITOR."))?;
    let command = shlex::split(&configured)
        .filter(|segments| !segments.is_empty())
        .ok_or_else(|| anyhow!("Failed to parse external editor command: {configured}"))?;
    Ok(command)
}

pub(super) fn run_external_editor(seed: &str, editor_command: &[String]) -> Result<String> {
    let file = NamedTempFile::new().context("create external editor temp file")?;
    stdfs::write(file.path(), seed).context("seed external editor temp file")?;

    let (program, args) = editor_command
        .split_first()
        .ok_or_else(|| anyhow!("External editor command is empty"))?;
    let status = ProcessCommand::new(program)
        .args(args)
        .arg(file.path())
        .status()
        .with_context(|| format!("launch external editor `{program}`"))?;
    if !status.success() {
        return Err(anyhow!(
            "External editor exited with status {}",
            status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        ));
    }

    stdfs::read_to_string(file.path()).context("read external editor output")
}

pub(super) fn queued_command_preview(command: &RuntimeCommand) -> String {
    match command {
        RuntimeCommand::Prompt { message, .. } => {
            let preview = message_operator_text(message);
            format!("running prompt: {}", state::preview_text(&preview, 40))
        }
        RuntimeCommand::Steer { message, .. } => {
            format!("applying steer: {}", state::preview_text(message, 40))
        }
    }
}

pub(super) fn format_side_question_inspector(outcome: &SideQuestionOutcome) -> Vec<InspectorEntry> {
    vec![
        InspectorEntry::section("Side Question"),
        InspectorEntry::field("Command", format!("/btw {}", outcome.question)),
        InspectorEntry::section("Answer"),
        InspectorEntry::Plain(outcome.response.clone()),
    ]
}

pub(super) fn pending_control_kind_label(
    kind: crate::interaction::PendingControlKind,
) -> &'static str {
    match kind {
        crate::interaction::PendingControlKind::Prompt => "prompt",
        crate::interaction::PendingControlKind::Steer => "steer",
    }
}

pub(super) fn composer_has_prompt_content(state: &TuiState) -> bool {
    !state.input.trim().is_empty() || !state.draft_attachments.is_empty()
}

pub(super) fn composer_requires_prompt_submission(state: &TuiState) -> bool {
    state.draft_attachments.iter().any(|attachment| {
        !matches!(
            attachment.kind,
            ComposerDraftAttachmentKind::LargePaste { .. }
        )
    })
}

pub(super) fn composer_uses_image_input(state: &TuiState) -> bool {
    state.draft_attachments.iter().any(|attachment| {
        matches!(
            attachment.kind,
            ComposerDraftAttachmentKind::LocalImage { .. }
                | ComposerDraftAttachmentKind::RemoteImage { .. }
        )
    })
}

pub(super) fn build_startup_inspector(session: &state::SessionSummary) -> Vec<InspectorEntry> {
    let mut lines = vec![
        InspectorEntry::section("Ready"),
        InspectorEntry::field("workspace", session.workspace_name.clone()),
        InspectorEntry::field("session ref", session.active_session_ref.clone()),
        InspectorEntry::field("agent session", session.root_agent_session_id.clone()),
        InspectorEntry::field(
            "model",
            format!("{} / {}", session.provider_label, session.model),
        ),
        InspectorEntry::field(
            "image input",
            if session.supports_image_input {
                "enabled"
            } else {
                "disabled"
            },
        ),
        InspectorEntry::field(
            "root",
            state::preview_text(&session.workspace_root.display().to_string(), 56),
        ),
        InspectorEntry::section("Next"),
        InspectorEntry::collection("/help [query]", Some("browse commands")),
        InspectorEntry::collection("/statusline", Some("choose footer items")),
        InspectorEntry::collection("/thinking [level]", Some("pick or set model effort")),
        InspectorEntry::collection("/theme [name]", Some("pick or set tui theme")),
        InspectorEntry::collection("/details", Some("toggle tool details")),
        InspectorEntry::collection(
            "/permissions [mode]",
            Some("inspect or switch sandbox mode"),
        ),
        InspectorEntry::collection("/queue", Some("browse pending prompts and steers")),
        InspectorEntry::collection("/sessions", Some("browse history")),
        InspectorEntry::collection("/agent_sessions", Some("inspect or resume agents")),
        InspectorEntry::collection("/spawn_task <role> <prompt>", Some("launch child agent")),
        InspectorEntry::collection("/new", Some("start fresh without deleting history")),
        InspectorEntry::section("Environment"),
        InspectorEntry::field(
            "store",
            format!(
                "{} ({} sessions)",
                session.store_label, session.stored_session_count
            ),
        ),
        InspectorEntry::field("permissions", session.permission_mode.as_str()),
        InspectorEntry::field("sandbox", session.sandbox_summary.clone()),
        InspectorEntry::field(
            "tools",
            format!(
                "{} local / {} mcp",
                session.startup_diagnostics.local_tool_count,
                session.startup_diagnostics.mcp_tool_count
            ),
        ),
        InspectorEntry::field(
            "plugins",
            format!(
                "{} enabled / {} total",
                session.startup_diagnostics.enabled_plugin_count,
                session.startup_diagnostics.total_plugin_count
            ),
        ),
        InspectorEntry::section("Git"),
        if !session.host_process_surfaces_allowed {
            InspectorEntry::field("branch", "disabled while host subprocesses are blocked")
        } else if session.git.available {
            InspectorEntry::field("branch", session.git.branch.clone())
        } else {
            InspectorEntry::field("branch", "unavailable")
        },
        if !session.host_process_surfaces_allowed {
            InspectorEntry::field("dirty", "unavailable while host subprocesses are blocked")
        } else {
            InspectorEntry::field(
                "dirty",
                format!(
                    "staged {}  modified {}  untracked {}",
                    session.git.staged, session.git.modified, session.git.untracked
                ),
            )
        },
        InspectorEntry::section("Diagnostics"),
        InspectorEntry::field(
            "mcp servers",
            session.startup_diagnostics.mcp_servers.len().to_string(),
        ),
    ];
    if let Some(warning) = &session.store_warning {
        lines.push(InspectorEntry::Muted(format!(
            "warning: {}",
            state::preview_text(warning, 72)
        )));
    }
    if !session.startup_diagnostics.warnings.is_empty() {
        lines.push(InspectorEntry::Muted(format!(
            "warning: {}",
            state::preview_text(&session.startup_diagnostics.warnings.join(" | "), 80)
        )));
    }
    if !session.startup_diagnostics.diagnostics.is_empty() {
        lines.push(InspectorEntry::Plain(format!(
            "diagnostic: {}",
            state::preview_text(&session.startup_diagnostics.diagnostics.join(" | "), 80)
        )));
    }
    lines
}

pub(super) fn build_permissions_inspector(
    snapshot: &SessionStartupSnapshot,
    turn_grants: &PermissionProfile,
    session_grants: &PermissionProfile,
) -> Vec<InspectorEntry> {
    let mut lines = vec![
        InspectorEntry::section("Permissions"),
        InspectorEntry::field("mode", snapshot.permission_mode.as_str()),
        InspectorEntry::field("default sandbox", snapshot.default_sandbox_summary.clone()),
        InspectorEntry::field("effective sandbox", snapshot.sandbox_summary.clone()),
        InspectorEntry::field(
            "host subprocesses",
            if snapshot.host_process_surfaces_allowed {
                "enabled"
            } else {
                "blocked until danger-full-access or a real sandbox backend is available"
            },
        ),
        InspectorEntry::section("Modes"),
        InspectorEntry::Command("/permissions default".to_string()),
        InspectorEntry::Command("/permissions danger-full-access".to_string()),
        InspectorEntry::section("Additional Grants"),
        InspectorEntry::field("turn", permission_profile_summary(turn_grants)),
        InspectorEntry::field("session", permission_profile_summary(session_grants)),
    ];
    if snapshot.permission_mode != SessionPermissionMode::Default {
        lines.push(InspectorEntry::Muted(
            "note: returning to `/permissions default` keeps request_permissions grants, but reapplies the configured base sandbox.".to_string(),
        ));
    }
    lines
}

pub(super) fn permission_profile_summary(profile: &PermissionProfile) -> String {
    let mut entries = Vec::new();
    if !profile.read_roots.is_empty() {
        entries.push(format!(
            "read {}",
            state::preview_text(&profile.read_roots.join(", "), 56)
        ));
    }
    if !profile.write_roots.is_empty() {
        entries.push(format!(
            "write {}",
            state::preview_text(&profile.write_roots.join(", "), 56)
        ));
    }
    if profile.network_full {
        entries.push("network full".to_string());
    }
    if !profile.network_domains.is_empty() {
        entries.push(format!(
            "domains {}",
            state::preview_text(&profile.network_domains.join(", "), 56)
        ));
    }
    if entries.is_empty() {
        "none".to_string()
    } else {
        entries.join(" · ")
    }
}

pub(super) fn build_command_error_view(input: &str, message: &str) -> Vec<InspectorEntry> {
    let mut lines = message
        .lines()
        .map(|line| InspectorEntry::Plain(line.to_string()))
        .collect::<Vec<_>>();
    let query = input
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .filter(|query| !query.is_empty());
    let palette = command_palette_lines_for(query);
    if !palette.is_empty() {
        lines.push(InspectorEntry::Empty);
        lines.extend(palette);
    }
    lines
}

pub(super) fn build_mcp_prompt_inspector(loaded: &LoadedMcpPrompt) -> Vec<InspectorEntry> {
    vec![
        InspectorEntry::section("MCP Prompt"),
        InspectorEntry::field("server", loaded.server_name.clone()),
        InspectorEntry::field("prompt", loaded.prompt_name.clone()),
        InspectorEntry::field("arguments", loaded.arguments_summary.clone()),
    ]
}

pub(super) fn build_mcp_resource_inspector(loaded: &LoadedMcpResource) -> Vec<InspectorEntry> {
    vec![
        InspectorEntry::section("MCP Resource"),
        InspectorEntry::field("server", loaded.server_name.clone()),
        InspectorEntry::field("uri", loaded.uri.clone()),
        InspectorEntry::field("mime", loaded.mime_summary.clone()),
    ]
}

pub(super) fn live_task_wait_notice_entry(outcome: &LiveTaskWaitOutcome) -> TranscriptEntry {
    let headline = format!("Background task {} finished", outcome.task_id);
    let details = live_task_wait_notice_details(outcome);
    match outcome.status {
        AgentStatus::Completed => TranscriptEntry::success_summary_details(headline, details),
        AgentStatus::Failed => TranscriptEntry::error_summary_details(headline, details),
        AgentStatus::Cancelled => TranscriptEntry::warning_summary_details(headline, details),
        _ => TranscriptEntry::shell_summary_details(headline, details),
    }
}

pub(super) fn live_task_wait_notice_details(
    outcome: &LiveTaskWaitOutcome,
) -> Vec<TranscriptShellDetail> {
    let mut details = vec![
        TranscriptShellDetail::Raw {
            text: format!("status {}", outcome.status),
            continuation: false,
        },
        TranscriptShellDetail::Raw {
            text: format!("summary {}", state::preview_text(&outcome.summary, 96)),
            continuation: false,
        },
        TranscriptShellDetail::Raw {
            text: "next enter steer / tab queue / /task inspect".to_string(),
            continuation: false,
        },
    ];
    if !outcome.claimed_files.is_empty() {
        details.push(TranscriptShellDetail::Raw {
            text: format!(
                "claimed files {}",
                state::preview_text(&outcome.claimed_files.join(", "), 96)
            ),
            continuation: false,
        });
    }
    if !outcome.remaining_live_tasks.is_empty() {
        details.push(TranscriptShellDetail::Raw {
            text: format!(
                "still running {}",
                state::preview_text(
                    &outcome
                        .remaining_live_tasks
                        .iter()
                        .map(|task| format!("{} ({}, {})", task.task_id, task.role, task.status))
                        .collect::<Vec<_>>()
                        .join(", "),
                    96
                )
            ),
            continuation: false,
        });
    }
    details
}

pub(super) fn live_task_wait_ui_toast_tone(outcome: &LiveTaskWaitOutcome) -> ToastTone {
    match outcome.status {
        AgentStatus::Completed => ToastTone::Success,
        AgentStatus::Failed => ToastTone::Error,
        AgentStatus::Cancelled => ToastTone::Warning,
        _ => ToastTone::Info,
    }
}

pub(super) fn live_task_wait_toast_message(
    outcome: &LiveTaskWaitOutcome,
    turn_running: bool,
) -> String {
    let next_step = if turn_running {
        "enter steer / tab queue / /task inspect"
    } else {
        "model follow-up queued / /task inspect"
    };
    let mut parts = vec![
        format!("task {} {}", outcome.task_id, outcome.status),
        state::preview_text(&outcome.summary, 64),
    ];
    if !outcome.remaining_live_tasks.is_empty() {
        parts.push(format!(
            "{} still running",
            outcome.remaining_live_tasks.len()
        ));
    }
    parts.push(next_step.to_string());
    parts.join(" · ")
}
