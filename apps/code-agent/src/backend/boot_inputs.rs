use agent::DriverActivationOutcome;
use agent::mcp::{McpServerConfig, McpTransportConfig};
use agent::types::HookRegistration;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(crate) struct DriverHostInputs {
    pub(crate) runtime_hooks: Vec<HookRegistration>,
    pub(crate) mcp_servers: Vec<McpServerConfig>,
    pub(crate) instructions: Vec<String>,
}

pub(crate) fn merge_driver_host_inputs(
    runtime_hooks: Vec<HookRegistration>,
    mcp_servers: Vec<McpServerConfig>,
    instructions: Vec<String>,
    driver_outcome: &DriverActivationOutcome,
) -> DriverHostInputs {
    let mut merged = DriverHostInputs {
        runtime_hooks,
        mcp_servers,
        instructions,
    };
    // Driver activations augment the same host inputs that declarative plugins
    // feed into boot. Keep that merge isolated so both foreground runtimes and
    // subagents consume one ordered view of hooks, MCP servers, and preamble.
    driver_outcome.extend_host_inputs(
        &mut merged.runtime_hooks,
        &mut merged.mcp_servers,
        &mut merged.instructions,
    );
    merged
}

pub(crate) fn resolve_mcp_servers(
    configs: &[McpServerConfig],
    workspace_root: &Path,
) -> Vec<McpServerConfig> {
    configs
        .iter()
        .cloned()
        .map(|mut server| {
            if let McpTransportConfig::Stdio { cwd, .. } = &mut server.transport
                && let Some(current_dir) = cwd.as_deref()
            {
                let resolved = resolve_path(workspace_root, current_dir);
                *cwd = Some(resolved.to_string_lossy().to_string());
            }
            server
        })
        .collect()
}

pub(crate) fn dedup_mcp_servers(servers: Vec<McpServerConfig>) -> Vec<McpServerConfig> {
    let mut by_name = BTreeMap::new();
    for server in servers {
        by_name.entry(server.name.clone()).or_insert(server);
    }
    by_name.into_values().collect()
}

fn resolve_path(base_dir: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}

#[cfg(test)]
pub(crate) fn driver_host_output_lines(driver_outcome: &DriverActivationOutcome) -> Vec<String> {
    driver_outcome
        .host_messages()
        .map(|message| match message.level {
            agent::DriverHostMessageLevel::Warning => {
                format!("warning: plugin driver warning: {}", message.message)
            }
            agent::DriverHostMessageLevel::Diagnostic => {
                format!("info: plugin driver diagnostic: {}", message.message)
            }
        })
        .collect()
}
