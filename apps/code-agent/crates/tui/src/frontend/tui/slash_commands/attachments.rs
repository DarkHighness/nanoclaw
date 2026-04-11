use super::*;

impl CodeAgentTui {
    pub(crate) async fn apply_attachment_command(&mut self, command: SlashCommand) -> Result<bool> {
        match command {
            SlashCommand::Image { path } => {
                self.attach_composer_image(&path).await;
                Ok(false)
            }
            SlashCommand::File { path } => {
                self.attach_composer_file(&path).await;
                Ok(false)
            }
            SlashCommand::Detach { index } => {
                self.detach_composer_attachment(index);
                Ok(false)
            }
            SlashCommand::MoveAttachment { from, to } => {
                self.move_composer_attachment(from, to);
                Ok(false)
            }
            _ => unreachable!("attachment handler received non-attachment command"),
        }
    }
}
