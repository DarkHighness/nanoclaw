use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use sched_claw::app_config::{CliOverrides, SchedClawConfig, app_state_dir};
use sched_claw::daemon_server::{ServeOptions, serve};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(disable_help_subcommand = true, subcommand_precedence_over_arg = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve(ServeArgs),
}

#[derive(Debug, Args)]
struct ServeArgs {
    #[arg(long, value_name = "PATH")]
    workspace_root: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    socket: Option<PathBuf>,
    #[arg(long = "allow-root", value_name = "PATH")]
    allow_roots: Vec<PathBuf>,
    #[arg(long, value_name = "LINES", default_value_t = 1_000)]
    log_capacity: usize,
    #[arg(long, value_name = "UID")]
    client_uid: Option<u32>,
    #[arg(long, value_name = "GID")]
    client_gid: Option<u32>,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();
    let args = match cli.command.unwrap_or(Command::Serve(ServeArgs {
        workspace_root: None,
        socket: None,
        allow_roots: Vec::new(),
        log_capacity: 1_000,
        client_uid: None,
        client_gid: None,
    })) {
        Command::Serve(args) => args,
    };
    let workspace_root = args.workspace_root.unwrap_or(std::env::current_dir()?);
    std::fs::create_dir_all(app_state_dir(&workspace_root)).with_context(|| {
        format!(
            "failed to create {}",
            app_state_dir(&workspace_root).display()
        )
    })?;
    let config = SchedClawConfig::load_from_dir(&workspace_root, &CliOverrides::default())?;
    let allow_roots = if args.allow_roots.is_empty() {
        vec![workspace_root.clone()]
    } else {
        args.allow_roots
    };
    let options = ServeOptions {
        workspace_root,
        socket_path: args.socket.unwrap_or(config.daemon.socket_path),
        allowed_roots: allow_roots,
        log_capacity: args.log_capacity.max(1),
        client_uid: args.client_uid.or_else(sudo_uid),
        client_gid: args.client_gid.or_else(sudo_gid),
    };
    serve(options).await
}

fn sudo_uid() -> Option<u32> {
    std::env::var("SUDO_UID").ok()?.parse().ok()
}

fn sudo_gid() -> Option<u32> {
    std::env::var("SUDO_GID").ok()?.parse().ok()
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .try_init();
}
