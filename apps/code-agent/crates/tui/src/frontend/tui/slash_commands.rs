use super::*;

mod attachments;
mod history;
mod live_task_commands;
mod live_tasks;
mod mcp;
mod session;

impl CodeAgentTui {
    pub(super) async fn apply_command(&mut self, input: &str) -> Result<bool> {
        match parse_slash_command(input) {
            SlashCommand::Quit => Ok(true),
            command @ (SlashCommand::Status
            | SlashCommand::Details
            | SlashCommand::StatusLine
            | SlashCommand::Thinking { .. }
            | SlashCommand::Theme { .. }
            | SlashCommand::Help { .. }
            | SlashCommand::Tools
            | SlashCommand::Skills
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
            command @ (SlashCommand::Mcp
            | SlashCommand::Prompts
            | SlashCommand::Resources
            | SlashCommand::Prompt { .. }
            | SlashCommand::Resource { .. }) => self.apply_mcp_command(command).await,
            command @ (SlashCommand::LiveTasks
            | SlashCommand::SpawnTask { .. }
            | SlashCommand::SendTask { .. }
            | SlashCommand::WaitTask { .. }
            | SlashCommand::CancelTask { .. }) => self.apply_live_task_command(command).await,
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
                let lines = build_command_error_view(input, &message);
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
