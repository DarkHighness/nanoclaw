use super::*;

impl CodeAgentSession {
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn startup_snapshot(&self) -> SessionStartupSnapshot {
        self.startup.read().unwrap().clone()
    }

    pub(super) fn connected_mcp_servers_snapshot(&self) -> Vec<ConnectedMcpServer> {
        self.mcp_servers.read().unwrap().clone()
    }

    pub(super) fn mcp_connection_failures_snapshot(&self) -> BTreeMap<String, String> {
        self.mcp_connection_failures.read().unwrap().clone()
    }

    pub(super) fn configured_mcp_server_summaries(
        &self,
        include_disabled: bool,
    ) -> Vec<McpServerSummary> {
        let configured = self
            .configured_mcp_servers
            .read()
            .unwrap()
            .iter()
            .filter(|server| include_disabled || server.enabled)
            .cloned()
            .collect::<Vec<_>>();
        summarize_mcp_servers(
            &configured,
            &self.connected_mcp_servers_snapshot(),
            &self.mcp_connection_failures_snapshot(),
        )
    }

    pub fn skill_summaries(&self) -> Vec<SkillSummary> {
        self.preamble
            .skill_catalog
            .all()
            .iter()
            .map(crate::frontend_contract::skill_summary_from_skill)
            .collect()
    }

    pub fn startup_diagnostics(&self) -> StartupDiagnosticsSnapshot {
        self.startup.read().unwrap().startup_diagnostics.clone()
    }

    pub fn cycle_model_reasoning_effort(&self) -> Result<ModelReasoningEffortOutcome> {
        let backend = self
            .model_backend
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("thinking effort controls are unavailable"))?;
        let update = backend.cycle_reasoning_effort()?;
        Ok(self.apply_model_reasoning_effort_update(update))
    }

    pub fn set_model_reasoning_effort(&self, effort: &str) -> Result<ModelReasoningEffortOutcome> {
        let backend = self
            .model_backend
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("thinking effort controls are unavailable"))?;
        let update = backend.set_reasoning_effort(effort)?;
        Ok(self.apply_model_reasoning_effort_update(update))
    }

    fn apply_model_reasoning_effort_update(
        &self,
        update: ReasoningEffortUpdate,
    ) -> ModelReasoningEffortOutcome {
        self.startup.write().unwrap().model_reasoning_effort = update.current.clone();
        ModelReasoningEffortOutcome {
            previous: update.previous,
            current: update.current,
            supported: update.supported,
        }
    }

    pub async fn list_mcp_servers(&self) -> Vec<McpServerSummary> {
        self.configured_mcp_server_summaries(false)
    }

    pub async fn list_mcp_prompts(&self) -> Vec<McpPromptSummary> {
        let servers = self.connected_mcp_servers_snapshot();
        list_mcp_prompts(&servers)
    }

    pub async fn list_mcp_resources(&self) -> Vec<McpResourceSummary> {
        let servers = self.connected_mcp_servers_snapshot();
        list_mcp_resources(&servers)
    }

    pub async fn load_mcp_prompt(
        &self,
        server_name: &str,
        prompt_name: &str,
    ) -> Result<LoadedMcpPrompt> {
        let servers = self.connected_mcp_servers_snapshot();
        load_mcp_prompt(&servers, server_name, prompt_name).await
    }

    pub async fn load_mcp_resource(
        &self,
        server_name: &str,
        uri: &str,
    ) -> Result<LoadedMcpResource> {
        let servers = self.connected_mcp_servers_snapshot();
        load_mcp_resource(&servers, server_name, uri).await
    }
}
