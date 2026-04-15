use super::state::{InspectorAction, InspectorEntry};
use crate::interaction::{SessionPermissionMode, SkillSummary};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};

mod completion;
mod parse;

#[cfg(test)]
pub(crate) use completion::{command_palette_lines, command_palette_lines_for};
pub(crate) use completion::{
    command_palette_lines_for_skills, composer_completion_hint, cycle_composer_completion,
    inspector_action_for_slash_name, move_composer_completion_selection,
    resolve_composer_enter_action,
};
#[cfg(test)]
pub(crate) use parse::parse_slash_command;
pub(crate) use parse::parse_slash_command_with_skills;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SlashCommandSpec {
    pub(crate) section: &'static str,
    pub(crate) name: &'static str,
    pub(crate) usage: &'static str,
    pub(crate) summary: &'static str,
}

impl SlashCommandSpec {
    pub(crate) fn requires_arguments(self) -> bool {
        self.argument_specs()
            .iter()
            .any(|argument| argument.required)
    }

    pub(crate) fn aliases(self) -> &'static [&'static str] {
        match self.name {
            "new" => &["clear"],
            "exit" => &["quit", "q"],
            _ => &[],
        }
    }

    pub(crate) fn matches_prefix(self, prefix: &str) -> bool {
        prefix.is_empty()
            || self.name.starts_with(prefix)
            || self.aliases().iter().any(|alias| alias.starts_with(prefix))
    }

    pub(crate) fn matches_token(self, token: &str) -> bool {
        self.name == token || self.aliases().contains(&token)
    }

    pub(crate) fn argument_specs(self) -> Vec<SlashCommandArgumentSpec> {
        self.usage
            .split_whitespace()
            .skip(1)
            .map(|placeholder| SlashCommandArgumentSpec {
                placeholder,
                required: placeholder.starts_with('<'),
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SlashCommandHint {
    pub(crate) selected: SlashInvocationSpec,
    pub(crate) matches: Vec<SlashInvocationSpec>,
    pub(crate) selected_match_index: usize,
    pub(crate) arguments: Option<SlashCommandArgumentHint>,
    pub(crate) exact: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SlashCommandArgumentHint {
    pub(crate) provided: Vec<SlashCommandArgumentValue>,
    pub(crate) next: Option<SlashCommandArgumentSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SlashCommandArgumentValue {
    pub(crate) placeholder: &'static str,
    pub(crate) value: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SlashCommandArgumentSpec {
    pub(crate) placeholder: &'static str,
    pub(crate) required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SlashCommandEnterAction {
    Complete { input: String, index: usize },
    Execute(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SlashInvocationSpec {
    Builtin(SlashCommandSpec),
    Skill(SkillInvocationSpec),
}

impl SlashInvocationSpec {
    pub(crate) fn name(&self) -> &str {
        match self {
            Self::Builtin(spec) => spec.name,
            Self::Skill(spec) => spec.name.as_str(),
        }
    }

    pub(crate) fn section(&self) -> &'static str {
        match self {
            Self::Builtin(spec) => spec.section,
            Self::Skill(_) => "Skills",
        }
    }

    pub(crate) fn usage(&self) -> String {
        match self {
            Self::Builtin(spec) => format!("/{}", spec.usage),
            Self::Skill(spec) => format!("/{} [prompt]", spec.name),
        }
    }

    pub(crate) fn summary(&self) -> String {
        match self {
            Self::Builtin(spec) => spec.summary.to_string(),
            Self::Skill(spec) => spec.description.clone(),
        }
    }

    pub(crate) fn aliases(&self) -> Vec<String> {
        match self {
            Self::Builtin(spec) => spec
                .aliases()
                .iter()
                .map(|alias| (*alias).to_string())
                .collect(),
            Self::Skill(spec) => spec.aliases.clone(),
        }
    }

    pub(crate) fn argument_specs(&self) -> Vec<SlashCommandArgumentSpec> {
        match self {
            Self::Builtin(spec) => spec.argument_specs(),
            Self::Skill(_) => Vec::new(),
        }
    }

    pub(crate) fn matches_prefix(&self, prefix: &str) -> bool {
        match self {
            Self::Builtin(spec) => spec.matches_prefix(prefix),
            Self::Skill(spec) => spec.matches_prefix(prefix),
        }
    }

    pub(crate) fn matches_token(&self, token: &str) -> bool {
        match self {
            Self::Builtin(spec) => spec.matches_token(token),
            Self::Skill(spec) => spec.matches_token(token),
        }
    }

    pub(crate) fn completion_input(&self) -> String {
        format!("/{} ", self.name())
    }

    pub(crate) fn executable_input(&self) -> Option<String> {
        match self {
            Self::Builtin(spec) if !spec.requires_arguments() => Some(format!("/{}", spec.name)),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SkillInvocationSpec {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) aliases: Vec<String>,
    pub(crate) tags: Vec<String>,
}

impl SkillInvocationSpec {
    pub(crate) fn from_summary(summary: &SkillSummary) -> Self {
        Self {
            name: summary.name.clone(),
            description: summary.description.clone(),
            aliases: summary.aliases.clone(),
            tags: summary.tags.clone(),
        }
    }

    pub(crate) fn invocation(&self) -> String {
        format!("${}", self.name)
    }

    pub(crate) fn matches_prefix(&self, prefix: &str) -> bool {
        let prefix = prefix.to_ascii_lowercase();
        prefix.is_empty()
            || self.name.to_ascii_lowercase().starts_with(&prefix)
            || self
                .aliases
                .iter()
                .any(|alias| alias.to_ascii_lowercase().starts_with(&prefix))
    }

    pub(crate) fn matches_token(&self, token: &str) -> bool {
        let token = token.to_ascii_lowercase();
        self.name.eq_ignore_ascii_case(&token)
            || self
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(&token))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SkillInvocationHint {
    pub(crate) selected: SkillInvocationSpec,
    pub(crate) matches: Vec<SkillInvocationSpec>,
    pub(crate) selected_match_index: usize,
    pub(crate) exact: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ComposerCompletionHint {
    Slash(SlashCommandHint),
    Skill(SkillInvocationHint),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ComposerCompletionEnterAction {
    Complete { input: String, index: usize },
    ExecuteSlash(String),
}

// Built-in slash commands are reserved for operator/session control surfaces.
// Model-visible tools must not be mirrored here unless the operator needs a
// distinct host control plane that a direct tool call cannot provide.
const SLASH_COMMAND_SPECS: &[SlashCommandSpec] = &[
    SlashCommandSpec {
        section: "Session",
        name: "help",
        usage: "help [query]",
        summary: "Browse commands",
    },
    SlashCommandSpec {
        section: "Session",
        name: "status",
        usage: "status",
        summary: "Session overview",
    },
    SlashCommandSpec {
        section: "Session",
        name: "details",
        usage: "details",
        summary: "Cycle tool detail levels",
    },
    SlashCommandSpec {
        section: "Session",
        name: "statusline",
        usage: "statusline",
        summary: "Toggle footer items",
    },
    SlashCommandSpec {
        section: "Session",
        name: "thinking",
        usage: "thinking [level]",
        summary: "Pick or set thinking effort",
    },
    SlashCommandSpec {
        section: "Session",
        name: "theme",
        usage: "theme [name]",
        summary: "Pick or set the TUI theme",
    },
    SlashCommandSpec {
        section: "Session",
        name: "motion",
        usage: "motion [on|off]",
        summary: "Toggle transcript intro motion",
    },
    SlashCommandSpec {
        section: "Session",
        name: "image",
        usage: "image <path-or-url>",
        summary: "Attach image to composer",
    },
    SlashCommandSpec {
        section: "Session",
        name: "file",
        usage: "file <path-or-url>",
        summary: "Attach file to composer",
    },
    SlashCommandSpec {
        section: "Session",
        name: "detach",
        usage: "detach [index]",
        summary: "Remove composer attachment",
    },
    SlashCommandSpec {
        section: "Session",
        name: "move-attachment",
        usage: "move-attachment <from> <to>",
        summary: "Reorder composer attachments",
    },
    SlashCommandSpec {
        section: "Session",
        name: "new",
        usage: "new",
        summary: "Fresh top-level session",
    },
    SlashCommandSpec {
        section: "Session",
        name: "compact",
        usage: "compact [notes]",
        summary: "Compact active history",
    },
    SlashCommandSpec {
        section: "Session",
        name: "btw",
        usage: "btw <question>",
        summary: "Ask a side question without interrupting work",
    },
    SlashCommandSpec {
        section: "Session",
        name: "steer",
        usage: "steer <notes>",
        summary: "Schedule safe-point guidance",
    },
    SlashCommandSpec {
        section: "Session",
        name: "queue",
        usage: "queue",
        summary: "Browse pending prompts and steers",
    },
    SlashCommandSpec {
        section: "Session",
        name: "permissions",
        usage: "permissions [default|danger-full-access]",
        summary: "Inspect or switch the session sandbox mode",
    },
    SlashCommandSpec {
        section: "Session",
        name: "exit",
        usage: "exit",
        summary: "Leave TUI",
    },
    SlashCommandSpec {
        section: "Agents",
        name: "live-tasks",
        usage: "live-tasks",
        summary: "List live child agents",
    },
    SlashCommandSpec {
        section: "Agents",
        name: "monitors",
        usage: "monitors [all]",
        summary: "List background monitors",
    },
    SlashCommandSpec {
        section: "Agents",
        name: "stop-monitor",
        usage: "stop-monitor <monitor-ref> [reason]",
        summary: "Stop background monitor",
    },
    SlashCommandSpec {
        section: "History",
        name: "sessions",
        usage: "sessions [query]",
        summary: "Browse persisted sessions",
    },
    SlashCommandSpec {
        section: "History",
        name: "session",
        usage: "session <session-ref>",
        summary: "Open persisted session",
    },
    SlashCommandSpec {
        section: "History",
        name: "agent-sessions",
        usage: "agent-sessions [session-ref]",
        summary: "List agent sessions",
    },
    SlashCommandSpec {
        section: "History",
        name: "agent-session",
        usage: "agent-session <agent-session-ref>",
        summary: "Inspect agent session",
    },
    SlashCommandSpec {
        section: "History",
        name: "resume",
        usage: "resume <agent-session-ref>",
        summary: "Reattach agent session",
    },
    SlashCommandSpec {
        section: "History",
        name: "tasks",
        usage: "tasks [session-ref]",
        summary: "List persisted child tasks",
    },
    SlashCommandSpec {
        section: "History",
        name: "task",
        usage: "task <task-id>",
        summary: "Inspect persisted task",
    },
    SlashCommandSpec {
        section: "Catalog",
        name: "diagnostics",
        usage: "diagnostics",
        summary: "Startup diagnostics",
    },
    SlashCommandSpec {
        section: "Catalog",
        name: "mcp",
        usage: "mcp",
        summary: "Toggle managed MCP servers",
    },
    SlashCommandSpec {
        section: "Catalog",
        name: "skill",
        usage: "skill",
        summary: "Toggle available skills",
    },
    SlashCommandSpec {
        section: "Catalog",
        name: "plugin",
        usage: "plugin",
        summary: "Toggle managed plugins",
    },
    SlashCommandSpec {
        section: "Catalog",
        name: "prompts",
        usage: "prompts",
        summary: "List MCP prompts",
    },
    SlashCommandSpec {
        section: "Catalog",
        name: "resources",
        usage: "resources",
        summary: "List MCP resources",
    },
    SlashCommandSpec {
        section: "Export",
        name: "export-events",
        usage: "export-events <session-ref> <path>",
        summary: "Write raw event export",
    },
    SlashCommandSpec {
        section: "Export",
        name: "export-transcript",
        usage: "export-transcript <session-ref> <path>",
        summary: "Write transcript export",
    },
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SlashCommand {
    Status,
    Details,
    StatusLine,
    Thinking {
        effort: Option<String>,
    },
    Theme {
        name: Option<String>,
    },
    Motion {
        enabled: Option<bool>,
    },
    Image {
        path: String,
    },
    File {
        path: String,
    },
    Detach {
        index: Option<usize>,
    },
    MoveAttachment {
        from: usize,
        to: usize,
    },
    Help {
        query: Option<String>,
    },
    Diagnostics,
    Mcp,
    Skill,
    Plugin,
    Prompts,
    Resources,
    Steer {
        message: Option<String>,
    },
    Queue,
    Permissions {
        mode: Option<SessionPermissionMode>,
    },
    Compact {
        notes: Option<String>,
    },
    Btw {
        question: Option<String>,
    },
    New,
    AgentSessions {
        session_ref: Option<String>,
    },
    AgentSession {
        agent_session_ref: String,
    },
    LiveTasks,
    Monitors {
        include_closed: bool,
    },
    StopMonitor {
        monitor_ref: String,
        reason: Option<String>,
    },
    Tasks {
        session_ref: Option<String>,
    },
    Task {
        task_ref: String,
    },
    Sessions {
        query: Option<String>,
    },
    Session {
        session_ref: String,
    },
    Resume {
        agent_session_ref: String,
    },
    ExportSession {
        session_ref: String,
        path: String,
    },
    ExportTranscript {
        session_ref: String,
        path: String,
    },
    InvokeSkill {
        skill_name: String,
        prompt: Option<String>,
    },
    Quit,
    InvalidUsage(String),
}

#[derive(Parser, Debug)]
#[command(
    no_binary_name = true,
    disable_version_flag = true,
    disable_help_flag = true,
    disable_help_subcommand = true
)]
struct SlashCli {
    #[command(subcommand)]
    command: SlashSubcommand,
}

#[derive(Subcommand, Debug)]
#[command(rename_all = "kebab-case")]
enum SlashSubcommand {
    Status,
    Details,
    Statusline,
    Thinking {
        effort: Option<String>,
    },
    Theme {
        name: Option<String>,
    },
    Motion {
        enabled: Option<MotionToggleArg>,
    },
    Image {
        #[arg(value_name = "PATH_OR_URL", required = true, trailing_var_arg = true)]
        path: Vec<String>,
    },
    File {
        #[arg(value_name = "PATH_OR_URL", required = true, trailing_var_arg = true)]
        path: Vec<String>,
    },
    Detach {
        index: Option<usize>,
    },
    MoveAttachment {
        from: usize,
        to: usize,
    },
    Help {
        #[arg(
            value_name = "QUERY",
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        query: Vec<String>,
    },
    Diagnostics,
    Mcp,
    Skill,
    Plugin,
    Prompts,
    Resources,
    Steer {
        #[arg(
            value_name = "NOTES",
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        message: Vec<String>,
    },
    Queue,
    Permissions {
        #[arg(value_enum)]
        mode: Option<PermissionModeArg>,
    },
    Compact {
        #[arg(
            value_name = "NOTES",
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        notes: Vec<String>,
    },
    Btw {
        #[arg(
            value_name = "QUESTION",
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        question: Vec<String>,
    },
    #[command(alias = "clear")]
    New,
    AgentSessions {
        session_ref: Option<String>,
    },
    AgentSession {
        agent_session_ref: String,
    },
    LiveTasks,
    Monitors {
        #[arg(value_name = "ALL")]
        include_closed: Vec<String>,
    },
    StopMonitor {
        monitor_ref: String,
        #[arg(
            value_name = "REASON",
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        reason: Vec<String>,
    },
    Tasks {
        session_ref: Option<String>,
    },
    Task {
        task_ref: String,
    },
    Sessions {
        #[arg(
            value_name = "QUERY",
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        query: Vec<String>,
    },
    Session {
        session_ref: String,
    },
    Resume {
        agent_session_ref: String,
    },
    #[command(name = "export-events")]
    ExportSession {
        session_ref: String,
        #[arg(value_name = "PATH", required = true, trailing_var_arg = true)]
        path: Vec<String>,
    },
    #[command(name = "export-transcript")]
    ExportTranscript {
        session_ref: String,
        #[arg(value_name = "PATH", required = true, trailing_var_arg = true)]
        path: Vec<String>,
    },
    #[command(name = "exit", alias = "quit", alias = "q")]
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "snake_case")]
enum MotionToggleArg {
    On,
    Off,
}

impl MotionToggleArg {
    fn enabled(self) -> bool {
        match self {
            Self::On => true,
            Self::Off => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum PermissionModeArg {
    Default,
    #[value(
        name = "danger-full-access",
        alias = "dangerous-full-access",
        alias = "danger"
    )]
    DangerFullAccess,
}

impl From<PermissionModeArg> for SessionPermissionMode {
    fn from(value: PermissionModeArg) -> Self {
        match value {
            PermissionModeArg::Default => SessionPermissionMode::Default,
            PermissionModeArg::DangerFullAccess => SessionPermissionMode::DangerFullAccess,
        }
    }
}

#[cfg(test)]
mod tests;
