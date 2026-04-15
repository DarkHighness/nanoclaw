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
        InspectorEntry::field("Workspace", session.workspace_name.clone()),
        InspectorEntry::field("Session Ref", session.active_session_ref.clone()),
        InspectorEntry::field("Agent Session", session.root_agent_session_id.clone()),
        InspectorEntry::field(
            "Model",
            format!("{} / {}", session.provider_label, session.model),
        ),
        InspectorEntry::field(
            "Image Input",
            if session.supports_image_input {
                "Enabled"
            } else {
                "Disabled"
            },
        ),
        InspectorEntry::field(
            "Root",
            state::preview_text(&session.workspace_root.display().to_string(), 56),
        ),
        InspectorEntry::section("Next"),
        command_entry("help", "/help [query]", "Browse commands"),
        command_entry("statusline", "/statusline", "Choose footer items"),
        command_entry("thinking", "/thinking [level]", "Pick or set model effort"),
        command_entry("theme", "/theme [name]", "Pick or set TUI theme"),
        command_entry("details", "/details", "Cycle tool detail levels"),
        command_entry(
            "permissions",
            "/permissions [mode]",
            "Inspect or switch sandbox mode",
        ),
        command_entry("mcp", "/mcp", "Toggle managed MCP servers"),
        command_entry("skill", "/skill", "Toggle available skills"),
        command_entry("plugin", "/plugin", "Toggle managed plugins"),
        command_entry("queue", "/queue", "Browse pending prompts and steers"),
        command_entry("sessions", "/sessions", "Browse history"),
        command_entry(
            "agent-sessions",
            "/agent-sessions",
            "Inspect or resume agents",
        ),
        command_entry("live-tasks", "/live-tasks", "Inspect active child agents"),
        command_entry("new", "/new", "Start fresh without deleting history"),
        InspectorEntry::section("Environment"),
        InspectorEntry::field(
            "Store",
            format!(
                "{} ({} sessions)",
                session.store_label, session.stored_session_count
            ),
        ),
        InspectorEntry::field("Permissions", session.permission_mode.as_str()),
        InspectorEntry::field("Sandbox", session.sandbox_summary.clone()),
        InspectorEntry::field(
            "Tools",
            format!(
                "{} Local / {} MCP",
                session.startup_diagnostics.local_tool_count,
                session.startup_diagnostics.mcp_tool_count
            ),
        ),
        InspectorEntry::field(
            "Plugins",
            format!(
                "{} Enabled / {} Total",
                session.startup_diagnostics.enabled_plugin_count,
                session.startup_diagnostics.total_plugin_count
            ),
        ),
        InspectorEntry::section("Git"),
        if !session.host_process_surfaces_allowed {
            InspectorEntry::field("Branch", "Disabled while host subprocesses are blocked")
        } else if session.git.available {
            InspectorEntry::field("Branch", session.git.branch.clone())
        } else {
            InspectorEntry::field("Branch", "Unavailable")
        },
        if !session.host_process_surfaces_allowed {
            InspectorEntry::field("Dirty", "Unavailable while host subprocesses are blocked")
        } else {
            InspectorEntry::field(
                "Dirty",
                format!(
                    "Staged {}  Modified {}  Untracked {}",
                    session.git.staged, session.git.modified, session.git.untracked
                ),
            )
        },
        InspectorEntry::section("Diagnostics"),
        InspectorEntry::field(
            "MCP Servers",
            session.startup_diagnostics.mcp_servers.len().to_string(),
        ),
    ];
    if let Some(warning) = &session.store_warning {
        lines.push(InspectorEntry::Muted(format!(
            "Warning: {}",
            state::preview_text(warning, 72)
        )));
    }
    if !session.startup_diagnostics.warnings.is_empty() {
        lines.push(InspectorEntry::Muted(format!(
            "Warning: {}",
            state::preview_text(&session.startup_diagnostics.warnings.join(" | "), 80)
        )));
    }
    if !session.startup_diagnostics.diagnostics.is_empty() {
        lines.push(InspectorEntry::Plain(format!(
            "Diagnostic: {}",
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
        InspectorEntry::field("Mode", snapshot.permission_mode.as_str()),
        InspectorEntry::field("Default Sandbox", snapshot.default_sandbox_summary.clone()),
        InspectorEntry::field("Effective Sandbox", snapshot.sandbox_summary.clone()),
        InspectorEntry::field(
            "Host Subprocesses",
            if snapshot.host_process_surfaces_allowed {
                "Enabled"
            } else {
                "Blocked until danger-full-access or a real sandbox backend is available"
            },
        ),
        InspectorEntry::section("Modes"),
        InspectorEntry::Command("/permissions default".to_string()),
        InspectorEntry::Command("/permissions danger-full-access".to_string()),
        InspectorEntry::section("Additional Grants"),
        InspectorEntry::field("Turn", permission_profile_summary(turn_grants)),
        InspectorEntry::field("Session", permission_profile_summary(session_grants)),
    ];
    if snapshot.permission_mode != SessionPermissionMode::Default {
        lines.push(InspectorEntry::Muted(
            "Note: returning to `/permissions default` keeps request_permissions grants, but reapplies the configured base sandbox.".to_string(),
        ));
    }
    lines
}

pub(crate) fn permission_profile_summary(profile: &PermissionProfile) -> String {
    let mut entries = Vec::new();
    if !profile.read_roots.is_empty() {
        entries.push(format!(
            "Read {}",
            state::preview_text(&profile.read_roots.join(", "), 56)
        ));
    }
    if !profile.write_roots.is_empty() {
        entries.push(format!(
            "Write {}",
            state::preview_text(&profile.write_roots.join(", "), 56)
        ));
    }
    if profile.network_full {
        entries.push("Network full".to_string());
    }
    if !profile.network_domains.is_empty() {
        entries.push(format!(
            "Domains {}",
            state::preview_text(&profile.network_domains.join(", "), 56)
        ));
    }
    if entries.is_empty() {
        "None".to_string()
    } else {
        entries.join(" · ")
    }
}

pub(crate) fn build_command_error_view(
    input: &str,
    message: &str,
    skills: &[crate::interaction::SkillSummary],
) -> Vec<InspectorEntry> {
    let mut lines = message
        .lines()
        .map(|line| InspectorEntry::Plain(line.to_string()))
        .collect::<Vec<_>>();
    let query = input
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .filter(|query| !query.is_empty());
    let palette = command_palette_lines_for_skills(query, skills);
    if !palette.is_empty() {
        lines.push(InspectorEntry::Empty);
        lines.extend(palette);
    }
    lines
}

pub(crate) fn build_mcp_prompt_inspector(loaded: &LoadedMcpPrompt) -> Vec<InspectorEntry> {
    vec![
        InspectorEntry::section("MCP Prompt"),
        InspectorEntry::field("Server", loaded.server_name.clone()),
        InspectorEntry::field("Prompt", loaded.prompt_name.clone()),
        InspectorEntry::field("Arguments", loaded.arguments_summary.clone()),
    ]
}

pub(crate) fn build_mcp_resource_inspector(loaded: &LoadedMcpResource) -> Vec<InspectorEntry> {
    vec![
        InspectorEntry::section("MCP Resource"),
        InspectorEntry::field("Server", loaded.server_name.clone()),
        InspectorEntry::field("URI", loaded.uri.clone()),
        InspectorEntry::field("MIME", loaded.mime_summary.clone()),
    ]
}
