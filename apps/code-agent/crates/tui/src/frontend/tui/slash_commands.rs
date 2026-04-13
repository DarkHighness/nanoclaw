use super::*;

mod attachments;
mod history;
mod live_tasks;
mod mcp;
mod runtime_activity_commands;
mod session;

impl CodeAgentTui {
    pub(super) async fn apply_command(&mut self, input: &str) -> Result<bool> {
        let skills = self.ui_state.snapshot().session.skills;
        match parse_slash_command_with_skills(input, &skills) {
            SlashCommand::Quit => Ok(true),
            SlashCommand::InvokeSkill { skill_name, prompt } => {
                self.apply_skill_slash_submit(skill_name, prompt).await;
                Ok(false)
            }
            command @ (SlashCommand::Status
            | SlashCommand::Details
            | SlashCommand::StatusLine
            | SlashCommand::Thinking { .. }
            | SlashCommand::Theme { .. }
            | SlashCommand::Motion { .. }
            | SlashCommand::Help { .. }
            | SlashCommand::Diagnostics
            | SlashCommand::Steer { .. }
            | SlashCommand::Queue
            | SlashCommand::Permissions { .. }
            | SlashCommand::New
            | SlashCommand::Compact { .. }
            | SlashCommand::Btw { .. }) => self.apply_session_command(command).await,
            command @ (SlashCommand::Image { .. }
            | SlashCommand::File { .. }
            | SlashCommand::Detach { .. }
            | SlashCommand::MoveAttachment { .. }) => self.apply_attachment_command(command).await,
            command @ (SlashCommand::Mcp | SlashCommand::Prompts | SlashCommand::Resources) => {
                self.apply_mcp_command(command).await
            }
            command @ (SlashCommand::LiveTasks
            | SlashCommand::Monitors { .. }
            | SlashCommand::StopMonitor { .. }) => {
                self.apply_runtime_activity_command(command).await
            }
            command @ (SlashCommand::AgentSessions { .. }
            | SlashCommand::AgentSession { .. }
            | SlashCommand::Tasks { .. }
            | SlashCommand::Task { .. }
            | SlashCommand::Sessions { .. }
            | SlashCommand::Session { .. }
            | SlashCommand::Resume { .. }
            | SlashCommand::ExportSession { .. }
            | SlashCommand::ExportTranscript { .. }) => self.apply_history_command(command).await,
            SlashCommand::InvalidUsage(message) => {
                let lines = build_command_error_view(input, &message, &skills);
                self.ui_state.mutate(|state| {
                    state.status = "Command syntax error".to_string();
                    state.show_main_view("Command Error", lines);
                    state.push_activity("command parse error");
                });
                Ok(false)
            }
        }
    }
}
