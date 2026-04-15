use agent::types::{Message, MessagePart};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StartupDiagnosticsSnapshot {
    pub local_tool_count: usize,
    pub mcp_tool_count: usize,
    pub enabled_plugin_count: usize,
    pub total_plugin_count: usize,
    pub mcp_servers: Vec<McpServerSummary>,
    pub plugin_details: Vec<String>,
    pub warnings: Vec<String>,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpServerSummary {
    pub server_name: String,
    pub transport: String,
    pub enabled: bool,
    pub connected: bool,
    pub tool_count: usize,
    pub prompt_count: usize,
    pub resource_count: usize,
    pub status_detail: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpPromptSummary {
    pub server_name: String,
    pub prompt_name: String,
    pub description: String,
    pub argument_names: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpResourceSummary {
    pub server_name: String,
    pub uri: String,
    pub mime_type: Option<String>,
    pub description: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadedMcpPrompt {
    pub input_text: String,
    pub input_messages: Vec<Message>,
    pub server_name: String,
    pub prompt_name: String,
    pub arguments_summary: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadedMcpResource {
    pub input_text: String,
    pub input_parts: Vec<MessagePart>,
    pub server_name: String,
    pub uri: String,
    pub mime_summary: String,
}
