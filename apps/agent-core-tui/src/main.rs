use anyhow::{Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let cwd = std::env::current_dir().context("failed to resolve current workspace")?;
    agent_core_tui::bootstrap_from_dir(&cwd)
        .await?
        .into_tui()
        .run()
        .await
}
