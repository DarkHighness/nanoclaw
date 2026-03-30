mod backend;
mod config;
mod frontend;
mod options;
mod preview;
mod provider;
mod statusline;
mod tool_render;

use crate::backend::{
    SandboxFallbackNotice, SessionApprovalMode, build_sandbox_fallback_notice, build_session,
    build_session_with_approval_mode, inject_process_env, inspect_sandbox_preflight,
};
use crate::frontend::startup_prompt::confirm_unsandboxed_startup_screen;
use crate::frontend::tui::{CodeAgentTui, SharedUiState};
use crate::options::AppOptions;
use agent::AgentWorkspaceLayout;
use agent::runtime::{HostRuntimeLimits, build_host_tokio_runtime};
use agent_env::EnvMap;
use anyhow::{Context, Result, bail};
use std::env;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    match try_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            print_fatal_error(&error);
            ExitCode::FAILURE
        }
    }
}

fn try_main() -> Result<()> {
    let workspace_root = env::current_dir().context("failed to resolve current workspace")?;
    let env_map = EnvMap::from_workspace_dir(&workspace_root)?;
    inject_process_env(&env_map);
    let _tracing_guard = init_tracing(&workspace_root)?;
    let options = AppOptions::from_env_and_args(&workspace_root, &env_map)?;

    let runtime = build_host_tokio_runtime(HostRuntimeLimits {
        worker_threads: options.tokio_worker_threads,
        max_blocking_threads: options.tokio_max_blocking_threads,
    })
    .context("failed to build tokio runtime")?;
    let local = tokio::task::LocalSet::new();
    runtime.block_on(local.run_until(async_main(workspace_root, options)))
}

fn print_fatal_error(error: &anyhow::Error) {
    let _ = writeln!(io::stderr().lock(), "error: {error}");
    if should_render_diagnostic_details(error) {
        let _ = writeln!(
            io::stderr().lock(),
            "\ninternal diagnostic report:\n{error:?}"
        );
    }
}

fn should_render_diagnostic_details(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.is::<agent::runtime::RuntimeError>()
            || cause.is::<agent::provider::ProviderError>()
            || cause.is::<agent::inference::InferenceError>()
    })
}

fn init_tracing(workspace_root: &Path) -> Result<WorkerGuard> {
    let layout = AgentWorkspaceLayout::new(workspace_root);
    layout.ensure_standard_layout().with_context(|| {
        format!(
            "failed to materialize workspace state layout at {}",
            layout.state_dir().display()
        )
    })?;
    let log_dir = layout.logs_dir();
    let file_appender = tracing_appender::rolling::never(log_dir, "code-agent.log");
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

async fn async_main(workspace_root: PathBuf, options: AppOptions) -> Result<()> {
    let mut options = options;
    let stdin_is_terminal = io::stdin().is_terminal();
    let stdout_is_terminal = io::stdout().is_terminal();
    if options.one_shot_prompt.is_none() && (!stdin_is_terminal || !stdout_is_terminal) {
        bail!(
            "code-agent requires a terminal for interactive mode; pass a prompt argument to run headless one-shot mode"
        );
    }
    confirm_unsandboxed_startup_if_needed(
        &workspace_root,
        &mut options,
        stdin_is_terminal && stdout_is_terminal,
    )?;
    // One-shot prompt invocations are also used from scripts and tests. When a
    // real terminal is unavailable, bypass the TUI so raw-mode setup does not
    // fail before the runtime can execute the prompt.
    if launch_headless_one_shot(&options, stdin_is_terminal, stdout_is_terminal) {
        return run_headless_one_shot(workspace_root, options).await;
    }

    let ui_state = SharedUiState::new();
    let session = build_session(&options, &workspace_root).await?;

    CodeAgentTui::new(session, options.one_shot_prompt.clone(), ui_state)
        .run()
        .await
}

fn launch_headless_one_shot(
    options: &AppOptions,
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
) -> bool {
    options.one_shot_prompt.is_some() && (!stdin_is_terminal || !stdout_is_terminal)
}

async fn run_headless_one_shot(workspace_root: PathBuf, options: AppOptions) -> Result<()> {
    let prompt = options
        .one_shot_prompt
        .clone()
        .context("headless one-shot mode requires a prompt")?;
    let session = build_session_with_approval_mode(
        &options,
        &workspace_root,
        SessionApprovalMode::NonInteractive,
    )
    .await?;
    let result = session.run_one_shot_prompt(&prompt).await;
    let end_reason = if result.is_ok() {
        "one_shot_complete"
    } else {
        "one_shot_failed"
    };
    let _ = session.end_session(Some(end_reason.to_string())).await;
    let outcome = result?;
    if !outcome.assistant_text.is_empty() {
        let mut stdout = io::stdout().lock();
        stdout.write_all(outcome.assistant_text.as_bytes())?;
        if !outcome.assistant_text.ends_with('\n') {
            stdout.write_all(b"\n")?;
        }
        stdout.flush()?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SandboxFallbackAction {
    Continue,
    Prompt,
    Abort,
}

fn confirm_unsandboxed_startup_if_needed(
    workspace_root: &Path,
    options: &mut AppOptions,
    interactive_terminal: bool,
) -> Result<()> {
    let preflight = inspect_sandbox_preflight(workspace_root, options);
    let Some(notice) = build_sandbox_fallback_notice(&preflight) else {
        return Ok(());
    };
    match choose_sandbox_fallback_action(options, interactive_terminal) {
        SandboxFallbackAction::Continue => {
            options.sandbox_fail_if_unavailable = false;
            print_sandbox_fallback_notice(
                &notice,
                "Continuing because --allow-no-sandbox was set.\n",
            )?;
            Ok(())
        }
        SandboxFallbackAction::Prompt => {
            if operator_confirms_unsandboxed_startup(&notice)? {
                options.sandbox_fail_if_unavailable = false;
                Ok(())
            } else {
                bail!("aborted because sandbox enforcement is unavailable on this host")
            }
        }
        SandboxFallbackAction::Abort => bail!(format_sandbox_abort_message(&notice)),
    }
}

fn choose_sandbox_fallback_action(
    options: &AppOptions,
    interactive_terminal: bool,
) -> SandboxFallbackAction {
    if options.allow_no_sandbox {
        SandboxFallbackAction::Continue
    } else if interactive_terminal {
        SandboxFallbackAction::Prompt
    } else {
        // Headless invocations cannot answer a startup risk prompt, so require
        // an explicit CLI override instead of silently inheriting host fallback.
        SandboxFallbackAction::Abort
    }
}

fn print_sandbox_fallback_notice(notice: &SandboxFallbackNotice, trailer: &str) -> Result<()> {
    let mut stderr = io::stderr().lock();
    writeln!(
        stderr,
        "warning: sandbox backend unavailable for the configured runtime policy"
    )?;
    writeln!(stderr, "  policy: {}", notice.policy_summary)?;
    writeln!(stderr, "  reason: {}", notice.reason)?;
    writeln!(stderr, "  risk: {}", notice.risk_summary)?;
    writeln!(stderr, "  setup:")?;
    for (index, step) in notice.setup_steps.iter().enumerate() {
        writeln!(stderr, "    {}. {}", index + 1, step)?;
    }
    write!(stderr, "{trailer}")?;
    stderr.flush()?;
    Ok(())
}

fn operator_confirms_unsandboxed_startup(notice: &SandboxFallbackNotice) -> Result<bool> {
    confirm_unsandboxed_startup_screen(notice)
        .context("failed to render sandbox confirmation screen")
}

fn format_sandbox_abort_message(notice: &SandboxFallbackNotice) -> String {
    let mut lines = vec![
        "sandbox backend unavailable for the configured runtime policy".to_string(),
        format!("policy: {}", notice.policy_summary),
        format!("reason: {}", notice.reason),
        format!("risk: {}", notice.risk_summary),
        "setup:".to_string(),
    ];
    lines.extend(
        notice
            .setup_steps
            .iter()
            .enumerate()
            .map(|(index, step)| format!("  {}. {}", index + 1, step)),
    );
    lines.push(
        "rerun in a terminal to confirm explicitly, or pass --allow-no-sandbox to accept the risk for this invocation".to_string(),
    );
    lines.join("\n")
}

#[cfg(test)]
mod diagnostic_tests {
    use super::should_render_diagnostic_details;
    use anyhow::anyhow;

    #[test]
    fn internal_runtime_errors_request_diagnostic_output() {
        let error = anyhow::Error::from(agent::runtime::RuntimeError::model_backend(
            "provider transport failed",
        ));
        assert!(should_render_diagnostic_details(&error));
    }

    #[test]
    fn plain_operator_errors_stay_concise() {
        let error = anyhow!("missing prompt");
        assert!(!should_render_diagnostic_details(&error));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SandboxFallbackAction, choose_sandbox_fallback_action, format_sandbox_abort_message,
        launch_headless_one_shot,
    };
    use crate::backend::{
        CodeAgentSubagentProfileResolver, SandboxFallbackNotice, build_sandbox_policy,
        dedup_mcp_servers, driver_host_output_lines, merge_driver_host_inputs, resolve_mcp_servers,
        tool_context_for_profile,
    };
    use crate::options::{AppOptions, parse_bool_flag};
    use agent::DriverActivationOutcome;
    use agent::ToolExecutionContext;
    use agent::mcp::{McpServerConfig, McpTransportConfig};
    use agent::runtime::SubagentProfileResolver;
    use agent::tools::{NetworkPolicy, SandboxMode};
    use agent::types::{HookEvent, HookHandler, HookRegistration, HttpHookHandler};
    use agent_env::EnvMap;
    use nanoclaw_config::{
        AgentProfileConfig, AgentSandboxMode, CoreConfig, ModelCapabilitiesConfig, ModelConfig,
    };
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex, OnceLock};
    use tempfile::tempdir;

    fn env_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn parses_boolean_flag_values() {
        assert!(parse_bool_flag("true").unwrap());
        assert!(!parse_bool_flag("off").unwrap());
        assert!(parse_bool_flag("1").unwrap());
        assert!(parse_bool_flag("maybe").is_err());
    }

    #[test]
    fn headless_one_shot_activates_only_for_non_tty_prompt_runs() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "OPENAI_API_KEY=test-key\n").unwrap();
        let env_map = EnvMap::from_workspace_dir(dir.path()).unwrap();
        let prompted =
            AppOptions::from_env_and_args_iter(dir.path(), &env_map, vec!["inspect".to_string()])
                .unwrap();
        let interactive =
            AppOptions::from_env_and_args_iter(dir.path(), &env_map, std::iter::empty::<String>())
                .unwrap();

        assert!(launch_headless_one_shot(&prompted, false, true));
        assert!(launch_headless_one_shot(&prompted, true, false));
        assert!(!launch_headless_one_shot(&prompted, true, true));
        assert!(!launch_headless_one_shot(&interactive, false, false));
        assert!(!launch_headless_one_shot(&interactive, true, true));
    }

    #[test]
    fn sandbox_fallback_requires_explicit_override_in_non_interactive_runs() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "OPENAI_API_KEY=test-key\n").unwrap();
        let env_map = EnvMap::from_workspace_dir(dir.path()).unwrap();
        let default =
            AppOptions::from_env_and_args_iter(dir.path(), &env_map, std::iter::empty::<String>())
                .unwrap();
        let override_allowed = AppOptions::from_env_and_args_iter(
            dir.path(),
            &env_map,
            vec!["--allow-no-sandbox".to_string()],
        )
        .unwrap();

        assert_eq!(
            choose_sandbox_fallback_action(&default, true),
            SandboxFallbackAction::Prompt
        );
        assert_eq!(
            choose_sandbox_fallback_action(&default, false),
            SandboxFallbackAction::Abort
        );
        assert_eq!(
            choose_sandbox_fallback_action(&override_allowed, false),
            SandboxFallbackAction::Continue
        );
    }

    #[test]
    fn sandbox_abort_message_includes_setup_guidance_and_override_hint() {
        let message = format_sandbox_abort_message(&SandboxFallbackNotice {
            policy_summary: "workspace-write, network off, best effort host fallback".to_string(),
            reason: "bwrap probe failed".to_string(),
            risk_summary: "local subprocesses may run on the host".to_string(),
            setup_steps: vec![
                "install bubblewrap".to_string(),
                "enable user namespaces".to_string(),
            ],
        });

        assert!(message.contains("policy: workspace-write"));
        assert!(message.contains("1. install bubblewrap"));
        assert!(message.contains("--allow-no-sandbox"));
    }

    #[test]
    fn driver_outcome_extends_code_agent_runtime_inputs() {
        let merged = merge_driver_host_inputs(
            vec![HookRegistration {
                name: "existing-hook".into(),
                event: HookEvent::Stop,
                matcher: None,
                handler: HookHandler::Http(HttpHookHandler {
                    url: "https://example.test/existing".to_string(),
                    method: "POST".to_string(),
                    headers: BTreeMap::new(),
                }),
                timeout_ms: None,
                execution: None,
            }],
            vec![McpServerConfig {
                name: "existing-mcp".into(),
                transport: McpTransportConfig::Stdio {
                    command: "stdio-server".to_string(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                    cwd: None,
                },
            }],
            vec!["existing instruction".to_string()],
            &DriverActivationOutcome {
                warnings: Vec::new(),
                hooks: vec![HookRegistration {
                    name: "driver-hook".into(),
                    event: HookEvent::SessionStart,
                    matcher: None,
                    handler: HookHandler::Http(HttpHookHandler {
                        url: "https://example.test/hook".to_string(),
                        method: "POST".to_string(),
                        headers: BTreeMap::new(),
                    }),
                    timeout_ms: Some(500),
                    execution: None,
                }],
                mcp_servers: vec![McpServerConfig {
                    name: "driver-mcp".into(),
                    transport: McpTransportConfig::StreamableHttp {
                        url: "https://example.test/mcp".to_string(),
                        headers: BTreeMap::new(),
                    },
                }],
                instructions: vec!["driver instruction".to_string()],
                diagnostics: vec!["prepared runtime".to_string()],
            },
        );

        assert_eq!(
            merged
                .runtime_hooks
                .iter()
                .map(|hook| hook.name.as_str())
                .collect::<Vec<_>>(),
            vec!["existing-hook", "driver-hook"]
        );
        assert_eq!(
            merged
                .mcp_servers
                .iter()
                .map(|server| server.name.as_str())
                .collect::<Vec<_>>(),
            vec!["existing-mcp", "driver-mcp"]
        );
        assert_eq!(
            merged.instructions,
            vec![
                "existing instruction".to_string(),
                "driver instruction".to_string()
            ]
        );
    }

    #[test]
    fn tool_context_for_read_only_profile_promotes_accessible_roots_and_disables_full_network() {
        let profile = CoreConfig::default()
            .with_override(|config| {
                config.agents.roles.insert(
                    "reviewer".to_string(),
                    AgentProfileConfig {
                        sandbox: Some(AgentSandboxMode::ReadOnly),
                        ..AgentProfileConfig::default()
                    },
                );
            })
            .resolve_subagent_profile(Some("reviewer"))
            .unwrap();
        let context = tool_context_for_profile(
            &ToolExecutionContext {
                workspace_root: PathBuf::from("/workspace"),
                worktree_root: Some(PathBuf::from("/worktree")),
                additional_roots: vec![PathBuf::from("/refs")],
                writable_roots: vec![PathBuf::from("/workspace/tmp")],
                exec_roots: vec![PathBuf::from("/workspace/bin")],
                network_policy: Some(NetworkPolicy::Full),
                workspace_only: false,
                ..Default::default()
            },
            &profile,
        );

        assert!(context.workspace_only);
        assert!(context.writable_roots.is_empty());
        assert_eq!(context.network_policy, Some(NetworkPolicy::Off));
        assert_eq!(
            context.read_only_roots,
            vec![
                PathBuf::from("/refs"),
                PathBuf::from("/workspace"),
                PathBuf::from("/workspace/bin"),
                PathBuf::from("/workspace/tmp"),
                PathBuf::from("/worktree"),
            ]
        );
    }

    #[test]
    fn subagent_profile_resolver_routes_role_profiles_and_honors_tool_capability() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "OPENAI_API_KEY=test-key\n").unwrap();
        let resolver = CodeAgentSubagentProfileResolver {
            core: CoreConfig::default().with_override(|config| {
                let base_model = config.models["gpt_5_4_default"].clone();
                config.models.insert(
                    "reviewer_no_tools".to_string(),
                    ModelConfig {
                        capabilities: ModelCapabilitiesConfig {
                            tool_calls: false,
                            ..base_model.capabilities.clone()
                        },
                        ..base_model
                    },
                );
                config.agents.roles.insert(
                    "reviewer".to_string(),
                    AgentProfileConfig {
                        model: Some("reviewer_no_tools".to_string()),
                        system_prompt: Some("Review only".to_string()),
                        sandbox: Some(AgentSandboxMode::ReadOnly),
                        ..AgentProfileConfig::default()
                    },
                );
            }),
            env_map: EnvMap::from_workspace_dir(dir.path()).unwrap(),
            base_tool_context: Arc::new(std::sync::RwLock::new(ToolExecutionContext {
                workspace_root: PathBuf::from("/workspace"),
                worktree_root: Some(PathBuf::from("/workspace")),
                workspace_only: true,
                ..Default::default()
            })),
            skill_catalog: agent::SkillCatalog::default(),
            plugin_instructions: vec!["Plugin instruction".to_string()],
        };

        let profile = resolver
            .resolve_profile(&agent::types::AgentTaskSpec {
                task_id: "review".to_string(),
                role: "reviewer".to_string(),
                prompt: "review".to_string(),
                steer: None,
                allowed_tools: Vec::new(),
                requested_write_set: Vec::new(),
                dependency_ids: Vec::new(),
                timeout_seconds: None,
            })
            .unwrap();

        assert_eq!(profile.profile_name, "roles.reviewer");
        assert!(!profile.supports_tool_calls);
        assert!(profile.instructions.join("\n").contains("Review only"));
        assert_eq!(
            profile.tool_context.model_context_window_tokens,
            Some(400_000)
        );
        assert_eq!(
            profile.tool_context.network_policy,
            Some(NetworkPolicy::Off)
        );
    }

    #[test]
    fn empty_driver_outcome_keeps_code_agent_runtime_inputs_stable() {
        let merged = merge_driver_host_inputs(
            vec![HookRegistration {
                name: "existing-hook".into(),
                event: HookEvent::Stop,
                matcher: None,
                handler: HookHandler::Http(HttpHookHandler {
                    url: "https://example.test/existing".to_string(),
                    method: "POST".to_string(),
                    headers: BTreeMap::new(),
                }),
                timeout_ms: None,
                execution: None,
            }],
            vec![McpServerConfig {
                name: "existing-mcp".into(),
                transport: McpTransportConfig::Stdio {
                    command: "stdio-server".to_string(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                    cwd: None,
                },
            }],
            vec!["existing instruction".to_string()],
            &DriverActivationOutcome::default(),
        );

        assert_eq!(
            merged
                .runtime_hooks
                .iter()
                .map(|hook| hook.name.as_str())
                .collect::<Vec<_>>(),
            vec!["existing-hook"]
        );
        assert_eq!(
            merged
                .mcp_servers
                .iter()
                .map(|server| server.name.as_str())
                .collect::<Vec<_>>(),
            vec!["existing-mcp"]
        );
        assert_eq!(
            merged.instructions,
            vec!["existing instruction".to_string()]
        );
    }

    #[test]
    fn driver_diagnostics_are_rendered_for_host_output() {
        let lines = driver_host_output_lines(&DriverActivationOutcome {
            warnings: vec!["slow startup".to_string()],
            hooks: Vec::new(),
            mcp_servers: Vec::new(),
            instructions: Vec::new(),
            diagnostics: vec!["validated wasm hook module".to_string()],
        });

        assert_eq!(
            lines,
            vec![
                "warning: plugin driver warning: slow startup".to_string(),
                "info: plugin driver diagnostic: validated wasm hook module".to_string(),
            ]
        );
    }

    #[test]
    fn resolve_and_dedup_plugin_mcp_servers_matches_host_boot_expectations() {
        let dir = tempdir().unwrap();
        let resolved = dedup_mcp_servers(resolve_mcp_servers(
            &[
                McpServerConfig {
                    name: "dup".into(),
                    transport: McpTransportConfig::Stdio {
                        command: "first".to_string(),
                        args: Vec::new(),
                        env: BTreeMap::new(),
                        cwd: Some("relative".to_string()),
                    },
                },
                McpServerConfig {
                    name: "dup".into(),
                    transport: McpTransportConfig::Stdio {
                        command: "second".to_string(),
                        args: Vec::new(),
                        env: BTreeMap::new(),
                        cwd: Some("ignored".to_string()),
                    },
                },
            ],
            dir.path(),
        ));

        assert_eq!(resolved.len(), 1);
        match &resolved[0].transport {
            McpTransportConfig::Stdio { command, cwd, .. } => {
                let expected_cwd = dir.path().join("relative");
                assert_eq!(command, "first");
                assert_eq!(
                    cwd.as_deref(),
                    Some(expected_cwd.to_string_lossy().as_ref())
                );
            }
            McpTransportConfig::StreamableHttp { .. } => {
                panic!("expected stdio transport");
            }
        }
    }

    #[tokio::test]
    async fn loads_sandbox_fail_closed_from_env_and_cli() {
        let _guard = env_test_lock().lock().unwrap();
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join(".env"),
            "OPENAI_API_KEY=test-key\nNANOCLAW_CORE_SANDBOX_FAIL_IF_UNAVAILABLE=false\n",
        )
        .unwrap();
        let env_map = EnvMap::from_workspace_dir(dir.path()).unwrap();
        let options = AppOptions::from_env_and_args_iter(
            dir.path(),
            &env_map,
            vec![
                "--sandbox-fail-if-unavailable".to_string(),
                "true".to_string(),
            ],
        )
        .unwrap();
        let tool_context = ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            worktree_root: Some(dir.path().to_path_buf()),
            workspace_only: true,
            ..Default::default()
        };

        let policy = build_sandbox_policy(&options, &tool_context);

        assert_eq!(policy.mode, SandboxMode::WorkspaceWrite);
        assert_eq!(policy.network, NetworkPolicy::Off);
        assert!(policy.fail_if_unavailable);
    }
}
