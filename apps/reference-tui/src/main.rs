use agent::AgentWorkspaceLayout;
use anyhow::{Context, Result};
use std::path::Path;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    let cwd = std::env::current_dir().context("failed to resolve current workspace")?;
    let _tracing_guard = init_tracing(&cwd)?;
    reference_tui::bootstrap_from_dir(&cwd)
        .await?
        .into_tui()
        .run()
        .await
}

fn init_tracing(workspace_root: &Path) -> Result<WorkerGuard> {
    let log_dir = AgentWorkspaceLayout::new(workspace_root).logs_dir();
    std::fs::create_dir_all(&log_dir).with_context(|| {
        format!(
            "failed to create tracing log directory at {}",
            log_dir.display()
        )
    })?;
    let file_appender = tracing_appender::rolling::never(log_dir, "reference-tui.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    let env_filter = EnvFilter::try_new(agent_env::log_filter_or_default(
        "info,runtime=debug,provider=debug",
    ))
    .context("failed to parse tracing filter")?;
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(env_filter)
        .with_writer(non_blocking)
        .try_init()
        .map_err(|error| anyhow::anyhow!("failed to initialize tracing subscriber: {error}"))?;
    Ok(guard)
}
