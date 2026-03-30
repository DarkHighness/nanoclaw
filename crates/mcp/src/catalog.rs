use rmcp::model::ClientCapabilities;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use types::{McpServerName, Message, MessagePart, ToolSpec};

pub type RmcpProtocolError = rmcp::ErrorData;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpPromptArgument {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpPrompt {
    pub name: String,
    pub title: Option<String>,
    pub description: String,
    #[serde(default)]
    pub arguments: Vec<McpPromptArgument>,
    #[serde(default)]
    pub messages: Vec<Message>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    pub title: Option<String>,
    pub description: String,
    pub mime_type: Option<String>,
    #[serde(default)]
    pub parts: Vec<MessagePart>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpCatalog {
    pub server_name: McpServerName,
    pub tools: Vec<ToolSpec>,
    pub prompts: Vec<McpPrompt>,
    pub resources: Vec<McpResource>,
}

#[derive(Clone)]
pub struct ConnectedMcpServer {
    pub server_name: McpServerName,
    pub client: Arc<dyn crate::McpClient>,
    pub catalog: McpCatalog,
}

#[must_use]
pub fn default_client_capabilities() -> ClientCapabilities {
    ClientCapabilities::default()
}
