use super::*;
use agent::CodeIntelBackend;

impl CodeAgentSession {
    pub async fn code_diagnostics(
        &self,
        path: Option<&str>,
    ) -> Result<Vec<agent::tools::CodeDiagnostic>> {
        let context = self.session_tool_context.read().unwrap().clone();
        let resolved_path = resolve_diagnostics_path(path, &context)?;
        self.code_intel_backend
            .diagnostics(resolved_path.as_deref(), 128, &context)
            .await
            .map_err(anyhow::Error::from)
    }
}

fn resolve_diagnostics_path(
    path: Option<&str>,
    context: &ToolExecutionContext,
) -> Result<Option<PathBuf>> {
    let Some(path) = path.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };

    let resolved = agent::tools::resolve_tool_path_against_workspace_root(
        path,
        context.effective_root(),
        context.container_workdir.as_deref(),
    )?;
    context
        .assert_path_read_allowed(&resolved)
        .map_err(anyhow::Error::from)?;
    Ok(Some(resolved))
}
