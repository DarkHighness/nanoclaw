use super::*;
use crate::frontend::tui::commands::inspector_action_for_slash_name;

pub(crate) fn format_side_question_inspector(outcome: &SideQuestionOutcome) -> Vec<InspectorEntry> {
    vec![
        InspectorEntry::section("Side Question"),
        InspectorEntry::field("Command", format!("/btw {}", outcome.question)),
        InspectorEntry::section("Answer"),
        InspectorEntry::Plain(outcome.response.clone()),
    ]
}

pub(crate) fn build_startup_inspector(session: &state::SessionSummary) -> Vec<InspectorEntry> {
    let command_entry = |name: &str, usage: &'static str, summary: &'static str| {
        inspector_action_for_slash_name(name)
            .map(|action| InspectorEntry::actionable_collection(usage, Some(summary), action))
            .unwrap_or_else(|| InspectorEntry::collection(usage, Some(summary)))
    };
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
        command_entry("help", "/help [query]", "browse commands"),
        command_entry("statusline", "/statusline", "choose footer items"),
        command_entry("thinking", "/thinking [level]", "pick or set model effort"),
        command_entry("theme", "/theme [name]", "pick or set tui theme"),
        command_entry("details", "/details", "toggle tool details"),
        command_entry(
            "permissions",
            "/permissions [mode]",
            "inspect or switch sandbox mode",
        ),
        command_entry("queue", "/queue", "browse pending prompts and steers"),
        command_entry("sessions", "/sessions", "browse history"),
        command_entry(
            "agent_sessions",
            "/agent_sessions",
            "inspect or resume agents",
        ),
        command_entry(
            "spawn_task",
            "/spawn_task <role> <prompt>",
            "launch child agent",
        ),
        command_entry("new", "/new", "start fresh without deleting history"),
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

pub(crate) fn build_permissions_inspector(
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

pub(crate) fn permission_profile_summary(profile: &PermissionProfile) -> String {
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

pub(crate) fn build_command_error_view(input: &str, message: &str) -> Vec<InspectorEntry> {
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

pub(crate) fn build_mcp_prompt_inspector(loaded: &LoadedMcpPrompt) -> Vec<InspectorEntry> {
    vec![
        InspectorEntry::section("MCP Prompt"),
        InspectorEntry::field("server", loaded.server_name.clone()),
        InspectorEntry::field("prompt", loaded.prompt_name.clone()),
        InspectorEntry::field("arguments", loaded.arguments_summary.clone()),
    ]
}

pub(crate) fn build_mcp_resource_inspector(loaded: &LoadedMcpResource) -> Vec<InspectorEntry> {
    vec![
        InspectorEntry::section("MCP Resource"),
        InspectorEntry::field("server", loaded.server_name.clone()),
        InspectorEntry::field("uri", loaded.uri.clone()),
        InspectorEntry::field("mime", loaded.mime_summary.clone()),
    ]
}
