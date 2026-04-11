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

    pub fn skill_summaries(&self) -> Vec<SkillSummary> {
        self.skills.iter().map(skill_summary_from_skill).collect()
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
        let servers = self.connected_mcp_servers_snapshot();
        list_mcp_servers(&servers)
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

    pub(super) fn rebuild_system_preamble(&self) -> Vec<String> {
        let tool_visibility = self
            .session_tool_context
            .read()
            .unwrap()
            .model_visibility
            .clone();
        build_system_preamble(
            self.workspace_root(),
            &self.preamble.profile,
            &self.preamble.skill_catalog,
            &self.preamble.plugin_instructions,
            &tool_visibility,
        )
    }
}
