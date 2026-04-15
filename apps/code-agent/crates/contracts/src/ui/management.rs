#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManagedMcpServerSummary {
    pub name: String,
    pub transport: String,
    pub enabled: bool,
    pub connected: bool,
    pub tool_count: usize,
    pub prompt_count: usize,
    pub resource_count: usize,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManagedSkillSummary {
    pub name: String,
    pub description: String,
    pub path: String,
    pub enabled: bool,
    pub builtin: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManagedPluginSummary {
    pub plugin_id: String,
    pub kind: String,
    pub path: String,
    pub enabled: bool,
    pub contribution_summary: String,
}
