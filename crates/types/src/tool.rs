use crate::{CallId, McpServerName, MessagePart, PluginId, ToolCallId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    borrow::Borrow,
    collections::{BTreeMap, BTreeSet},
    fmt,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutputMode {
    #[default]
    Text,
    ContentParts,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    #[default]
    Function,
    Freeform,
    Native,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolFreeformFormatKind {
    Grammar,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolFreeformFormat {
    #[serde(rename = "type")]
    pub kind: ToolFreeformFormatKind,
    pub syntax: String,
    pub definition: String,
}

impl ToolFreeformFormat {
    #[must_use]
    pub fn grammar(syntax: impl Into<String>, definition: impl Into<String>) -> Self {
        Self {
            kind: ToolFreeformFormatKind::Grammar,
            syntax: syntax.into(),
            definition: definition.into(),
        }
    }
}

/// Tool names are shared protocol identifiers across registry lookup,
/// approval policy, provider mapping, and persisted session events.
/// Keeping them
/// typed inside the substrate avoids repeatedly degrading them into raw
/// strings before crossing a real JSON or UI boundary.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct ToolName(String);

impl ToolName {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl From<String> for ToolName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ToolName {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl AsRef<str> for ToolName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for ToolName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ToolName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolOrigin {
    Local,
    Mcp { server_name: McpServerName },
    Provider { provider: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolSource {
    #[default]
    Builtin,
    Dynamic,
    Plugin {
        plugin: PluginId,
    },
    McpTool {
        server_name: McpServerName,
    },
    McpResource {
        server_name: McpServerName,
    },
    ProviderBuiltin {
        provider: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpTransportKind {
    Stdio,
    StreamableHttp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpToolBoundaryClass {
    LocalProcess,
    RemoteService,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpToolBoundary {
    pub transport: McpTransportKind,
    pub boundary_class: McpToolBoundaryClass,
}

impl McpToolBoundary {
    #[must_use]
    pub fn local_process(transport: McpTransportKind) -> Self {
        Self {
            transport,
            boundary_class: McpToolBoundaryClass::LocalProcess,
        }
    }

    #[must_use]
    pub fn remote_service(transport: McpTransportKind) -> Self {
        Self {
            transport,
            boundary_class: McpToolBoundaryClass::RemoteService,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default, JsonSchema)]
pub struct ToolAvailability {
    /// Feature flags gate tools on host/runtime capabilities that are not part
    /// of the provider or model identity.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub feature_flags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_allowlist: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_allowlist: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub role_allowlist: Vec<String>,
    #[serde(default)]
    pub hidden_from_model: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ToolVisibilityContext {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub role: Option<String>,
    pub features: BTreeSet<String>,
}

impl ToolVisibilityContext {
    #[must_use]
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = normalize_visibility_value(Some(provider.into()));
        self
    }

    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = normalize_visibility_value(Some(model.into()));
        self
    }

    #[must_use]
    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.role = normalize_visibility_value(Some(role.into()));
        self
    }

    #[must_use]
    pub fn with_feature(mut self, feature: impl Into<String>) -> Self {
        if let Some(feature) = normalize_visibility_value(Some(feature.into())) {
            self.features.insert(feature);
        }
        self
    }

    #[must_use]
    pub fn has_feature(&self, feature: &str) -> bool {
        self.features.contains(feature)
    }

    pub fn set_feature_enabled(&mut self, feature: &str, enabled: bool) {
        if !enabled {
            self.features.remove(feature);
            return;
        }

        if let Some(feature) = normalize_visibility_value(Some(feature.to_string())) {
            self.features.insert(feature);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DynamicToolSpec {
    pub name: ToolName,
    pub description: String,
    pub input_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freeform_format: Option<ToolFreeformFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freeform_availability: Option<ToolAvailability>,
    #[serde(default)]
    pub output_mode: ToolOutputMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(default)]
    pub defer_loading: bool,
    #[serde(default)]
    pub aliases: Vec<ToolName>,
    #[serde(default)]
    pub supports_parallel_tool_calls: bool,
    #[serde(default)]
    pub availability: ToolAvailability,
    #[serde(default)]
    pub approval: ToolApprovalProfile,
}

impl DynamicToolSpec {
    #[must_use]
    pub fn function(
        name: impl Into<ToolName>,
        description: impl Into<String>,
        input_schema: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            freeform_format: None,
            freeform_availability: None,
            output_mode: ToolOutputMode::Text,
            output_schema: None,
            defer_loading: false,
            aliases: Vec::new(),
            supports_parallel_tool_calls: false,
            availability: ToolAvailability::default(),
            approval: ToolApprovalProfile::default(),
        }
    }

    #[must_use]
    pub fn with_output_mode(mut self, output_mode: ToolOutputMode) -> Self {
        self.output_mode = output_mode;
        self
    }

    #[must_use]
    pub fn with_freeform_format(mut self, freeform_format: ToolFreeformFormat) -> Self {
        self.freeform_format = Some(freeform_format);
        self
    }

    #[must_use]
    pub fn with_freeform_availability(mut self, availability: ToolAvailability) -> Self {
        self.freeform_availability = Some(availability);
        self
    }

    #[must_use]
    pub fn with_output_schema(mut self, output_schema: Value) -> Self {
        self.output_schema = Some(output_schema);
        self
    }

    #[must_use]
    pub fn with_defer_loading(mut self, defer_loading: bool) -> Self {
        self.defer_loading = defer_loading;
        self
    }

    #[must_use]
    pub fn with_aliases(mut self, aliases: Vec<ToolName>) -> Self {
        self.aliases = aliases;
        self
    }

    #[must_use]
    pub fn with_parallel_support(mut self, supports_parallel_tool_calls: bool) -> Self {
        self.supports_parallel_tool_calls = supports_parallel_tool_calls;
        self
    }

    #[must_use]
    pub fn with_availability(mut self, availability: ToolAvailability) -> Self {
        self.availability = availability;
        self
    }

    #[must_use]
    pub fn with_approval(mut self, approval: ToolApprovalProfile) -> Self {
        self.approval = approval;
        self
    }

    #[must_use]
    pub fn into_tool_spec(self) -> ToolSpec {
        let mut tool = ToolSpec::function(
            self.name,
            self.description,
            self.input_schema,
            self.output_mode,
            ToolOrigin::Local,
            ToolSource::Dynamic,
        )
        .with_aliases(self.aliases)
        .with_parallel_support(self.supports_parallel_tool_calls)
        .with_availability(self.availability)
        .with_approval(self.approval)
        .with_defer_loading(self.defer_loading);
        if let Some(freeform_format) = self.freeform_format {
            tool = tool.with_freeform_format(freeform_format);
        }
        if let Some(freeform_availability) = self.freeform_availability {
            tool = tool.with_freeform_availability(freeform_availability);
        }
        if let Some(output_schema) = self.output_schema {
            tool = tool.with_output_schema(output_schema);
        }
        tool
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolApprovalProfile {
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub mutates_state: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotent: Option<bool>,
    #[serde(default)]
    pub open_world: bool,
    #[serde(default)]
    pub needs_network: bool,
    #[serde(default)]
    pub needs_host_escape: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_message: Option<String>,
}

impl Default for ToolApprovalProfile {
    fn default() -> Self {
        Self {
            read_only: false,
            mutates_state: true,
            idempotent: None,
            open_world: true,
            needs_network: false,
            needs_host_escape: false,
            approval_message: None,
        }
    }
}

impl ToolApprovalProfile {
    #[must_use]
    pub fn new(
        read_only: bool,
        mutates_state: bool,
        idempotent: Option<bool>,
        open_world: bool,
    ) -> Self {
        Self {
            read_only,
            mutates_state,
            idempotent,
            open_world,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn with_network(mut self, needs_network: bool) -> Self {
        self.needs_network = needs_network;
        self
    }

    #[must_use]
    pub fn with_host_escape(mut self, needs_host_escape: bool) -> Self {
        self.needs_host_escape = needs_host_escape;
        self
    }

    #[must_use]
    pub fn with_approval_message(mut self, approval_message: impl Into<String>) -> Self {
        self.approval_message = Some(approval_message.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: ToolName,
    pub description: String,
    #[serde(default)]
    pub kind: ToolKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freeform_format: Option<ToolFreeformFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freeform_availability: Option<ToolAvailability>,
    pub output_mode: ToolOutputMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(default)]
    pub defer_loading: bool,
    pub origin: ToolOrigin,
    #[serde(default)]
    pub source: ToolSource,
    #[serde(default)]
    pub aliases: Vec<ToolName>,
    #[serde(default)]
    pub supports_parallel_tool_calls: bool,
    #[serde(default)]
    pub availability: ToolAvailability,
    #[serde(default)]
    pub approval: ToolApprovalProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_boundary: Option<McpToolBoundary>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub mcp_server_boundaries: BTreeMap<McpServerName, McpToolBoundary>,
}

impl ToolSpec {
    #[must_use]
    pub fn function(
        name: impl Into<ToolName>,
        description: impl Into<String>,
        input_schema: Value,
        output_mode: ToolOutputMode,
        origin: ToolOrigin,
        source: ToolSource,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            kind: ToolKind::Function,
            input_schema: Some(input_schema),
            freeform_format: None,
            freeform_availability: None,
            output_mode,
            output_schema: None,
            defer_loading: false,
            origin,
            source,
            aliases: Vec::new(),
            supports_parallel_tool_calls: false,
            availability: ToolAvailability::default(),
            approval: ToolApprovalProfile::default(),
            mcp_boundary: None,
            mcp_server_boundaries: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_output_schema(mut self, output_schema: Value) -> Self {
        self.output_schema = Some(output_schema);
        self
    }

    #[must_use]
    pub fn freeform(
        name: impl Into<ToolName>,
        description: impl Into<String>,
        freeform_format: ToolFreeformFormat,
        output_mode: ToolOutputMode,
        origin: ToolOrigin,
        source: ToolSource,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            kind: ToolKind::Freeform,
            input_schema: None,
            freeform_format: Some(freeform_format),
            freeform_availability: None,
            output_mode,
            output_schema: None,
            defer_loading: false,
            origin,
            source,
            aliases: Vec::new(),
            supports_parallel_tool_calls: false,
            availability: ToolAvailability::default(),
            approval: ToolApprovalProfile::default(),
            mcp_boundary: None,
            mcp_server_boundaries: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_freeform_format(mut self, freeform_format: ToolFreeformFormat) -> Self {
        self.freeform_format = Some(freeform_format);
        self
    }

    #[must_use]
    pub fn with_freeform_availability(mut self, availability: ToolAvailability) -> Self {
        self.freeform_availability = Some(availability);
        self
    }

    #[must_use]
    pub fn with_defer_loading(mut self, defer_loading: bool) -> Self {
        self.defer_loading = defer_loading;
        self
    }

    #[must_use]
    pub fn with_approval(mut self, approval: ToolApprovalProfile) -> Self {
        self.approval = approval;
        self
    }

    #[must_use]
    pub fn with_mcp_boundary(mut self, boundary: McpToolBoundary) -> Self {
        self.mcp_boundary = Some(boundary);
        self
    }

    #[must_use]
    pub fn with_mcp_server_boundaries(
        mut self,
        boundaries: BTreeMap<McpServerName, McpToolBoundary>,
    ) -> Self {
        self.mcp_server_boundaries = boundaries;
        self
    }

    #[must_use]
    pub fn with_parallel_support(mut self, supports_parallel_tool_calls: bool) -> Self {
        self.supports_parallel_tool_calls = supports_parallel_tool_calls;
        self
    }

    #[must_use]
    pub fn with_aliases(mut self, aliases: Vec<ToolName>) -> Self {
        self.aliases = aliases;
        self
    }

    #[must_use]
    pub fn with_availability(mut self, availability: ToolAvailability) -> Self {
        self.availability = availability;
        self
    }

    #[must_use]
    pub fn is_model_visible(&self, context: &ToolVisibilityContext) -> bool {
        availability_matches_context(&self.availability, context, false)
    }

    #[must_use]
    pub fn supports_freeform_transport(&self, context: &ToolVisibilityContext) -> bool {
        if self.freeform_format.is_none() {
            return false;
        }

        match self.kind {
            ToolKind::Freeform => availability_matches_context(&self.availability, context, false),
            ToolKind::Function => self
                .freeform_availability
                .as_ref()
                .is_some_and(|availability| {
                    availability_matches_context(availability, context, true)
                }),
            ToolKind::Native => false,
        }
    }

    #[must_use]
    pub fn is_model_visible_for_provider(&self, provider_name: &str) -> bool {
        self.is_model_visible(&ToolVisibilityContext::default().with_provider(provider_name))
    }

    #[must_use]
    pub fn effective_mcp_boundary<'a>(&'a self, call: &ToolCall) -> Option<&'a McpToolBoundary> {
        if let Some(boundary) = self.mcp_boundary.as_ref() {
            return Some(boundary);
        }

        if let ToolOrigin::Mcp { server_name } = &call.origin {
            if let Some(boundary) = self.mcp_server_boundaries.get(server_name) {
                return Some(boundary);
            }
        }

        call.arguments
            .get("server_name")
            .and_then(Value::as_str)
            .and_then(|server_name| self.mcp_server_boundaries.get(server_name))
    }
}

fn normalize_visibility_value(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn availability_matches_context(
    availability: &ToolAvailability,
    context: &ToolVisibilityContext,
    ignore_hidden: bool,
) -> bool {
    if !ignore_hidden && availability.hidden_from_model {
        return false;
    }
    if !availability
        .feature_flags
        .iter()
        .all(|required| context.has_feature(required))
    {
        return false;
    }
    if !availability.provider_allowlist.is_empty()
        && context
            .provider
            .as_deref()
            .is_some_and(|provider| provider != "unknown")
        && !availability
            .provider_allowlist
            .iter()
            .any(|allowed| allowed == context.provider.as_ref().expect("provider checked"))
    {
        return false;
    }
    if !availability.model_allowlist.is_empty()
        && context.model.as_deref().is_some()
        && !availability.model_allowlist.iter().any(|allowed| {
            matches_model_allowlist(
                allowed,
                context.model.as_ref().expect("model checked").as_str(),
            )
        })
    {
        return false;
    }
    if availability.role_allowlist.is_empty() {
        return true;
    }
    context.role.as_deref().is_some_and(|role| {
        availability
            .role_allowlist
            .iter()
            .any(|allowed| allowed == role)
    })
}

fn matches_model_allowlist(pattern: &str, model: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return false;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return model.starts_with(prefix);
    }
    model == pattern
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: ToolCallId,
    #[serde(default = "CallId::new")]
    pub call_id: CallId,
    pub tool_name: ToolName,
    pub arguments: Value,
    pub origin: ToolOrigin,
}

/// Tool continuations carry the stable machine-readable cursor that a follow-up
/// call should reuse instead of scraping prose from the transcript.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolContinuation {
    FileWindow {
        snapshot_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selection_hash: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_start_line: Option<usize>,
    },
    StreamWindow {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stdout_start_char: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stderr_start_char: Option<usize>,
    },
    DocumentWindow {
        document_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_start_index: Option<usize>,
    },
}

/// Attachments describe side-band artifacts that hosts may render or persist
/// without forcing every provider transport to understand the local message-part
/// variants directly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolAttachment {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub id: ToolCallId,
    #[serde(default = "CallId::new")]
    pub call_id: CallId,
    pub tool_name: ToolName,
    pub parts: Vec<MessagePart>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ToolAttachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<ToolContinuation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    pub is_error: bool,
}

impl ToolResult {
    #[must_use]
    pub fn text(id: ToolCallId, tool_name: impl Into<ToolName>, text: impl Into<String>) -> Self {
        Self {
            id,
            call_id: CallId::new(),
            tool_name: tool_name.into(),
            parts: vec![MessagePart::text(text)],
            attachments: Vec::new(),
            structured_content: None,
            continuation: None,
            metadata: None,
            is_error: false,
        }
    }

    #[must_use]
    pub fn error(id: ToolCallId, tool_name: impl Into<ToolName>, text: impl Into<String>) -> Self {
        Self {
            id,
            call_id: CallId::new(),
            tool_name: tool_name.into(),
            parts: vec![MessagePart::text(text)],
            attachments: Vec::new(),
            structured_content: None,
            continuation: None,
            metadata: None,
            is_error: true,
        }
    }

    #[must_use]
    pub fn with_call_id(mut self, call_id: impl Into<CallId>) -> Self {
        self.call_id = call_id.into();
        self
    }

    #[must_use]
    pub fn with_structured_content(mut self, structured_content: Value) -> Self {
        self.structured_content = Some(structured_content);
        self
    }

    #[must_use]
    pub fn with_continuation(mut self, continuation: ToolContinuation) -> Self {
        self.continuation = Some(continuation);
        self
    }

    #[must_use]
    pub fn with_attachment(mut self, attachment: ToolAttachment) -> Self {
        self.attachments.push(attachment);
        self
    }

    #[must_use]
    pub fn text_content(&self) -> String {
        self.parts
            .iter()
            .filter_map(|part| match part {
                MessagePart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        McpToolBoundary, McpTransportKind, ToolApprovalProfile, ToolAvailability, ToolCall,
        ToolCallId, ToolFreeformFormat, ToolName, ToolOrigin, ToolOutputMode, ToolSource, ToolSpec,
        ToolVisibilityContext,
    };
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn local_tool_spec_serialization_is_pinned() {
        let spec = ToolSpec::function(
            "inspect_workspace",
            "Inspect one workspace file",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"}
                },
                "required": ["path"]
            }),
            ToolOutputMode::Text,
            ToolOrigin::Local,
            ToolSource::Builtin,
        )
        .with_output_schema(json!({
            "type": "object",
            "properties": {
                "summary": {"type": "string"}
            },
            "required": ["summary"]
        }))
        .with_defer_loading(true)
        .with_aliases(vec![ToolName::from("inspect"), ToolName::from("peek")])
        .with_parallel_support(true)
        .with_availability(ToolAvailability {
            feature_flags: vec!["managed-lsp".to_string()],
            provider_allowlist: vec!["openai".to_string()],
            model_allowlist: vec!["gpt-5*".to_string()],
            role_allowlist: vec!["worker".to_string()],
            hidden_from_model: false,
        })
        .with_approval(
            ToolApprovalProfile::new(true, false, Some(true), false)
                .with_network(true)
                .with_approval_message("Requires trusted docs endpoint"),
        );

        assert_eq!(
            serde_json::to_value(&spec).unwrap(),
            json!({
                "name": "inspect_workspace",
                "description": "Inspect one workspace file",
                "kind": "function",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"}
                    },
                    "required": ["path"]
                },
                "output_mode": "text",
                "output_schema": {
                    "type": "object",
                    "properties": {
                        "summary": {"type": "string"}
                    },
                    "required": ["summary"]
                },
                "defer_loading": true,
                "origin": {"kind": "local"},
                "source": {"kind": "builtin"},
                "aliases": ["inspect", "peek"],
                "supports_parallel_tool_calls": true,
                "availability": {
                    "feature_flags": ["managed-lsp"],
                    "provider_allowlist": ["openai"],
                    "model_allowlist": ["gpt-5*"],
                    "role_allowlist": ["worker"],
                    "hidden_from_model": false
                },
                "approval": {
                    "read_only": true,
                    "mutates_state": false,
                    "idempotent": true,
                    "open_world": false,
                    "needs_network": true,
                    "needs_host_escape": false,
                    "approval_message": "Requires trusted docs endpoint"
                }
            })
        );
    }

    #[test]
    fn direct_mcp_boundary_is_resolved_from_tool_spec() {
        let call = ToolCall {
            id: ToolCallId::new(),
            call_id: "call-1".into(),
            tool_name: "inspect_context".into(),
            arguments: json!({}),
            origin: ToolOrigin::Mcp {
                server_name: "fixture".into(),
            },
        };
        let spec = ToolSpec::function(
            "inspect_context",
            "inspect",
            json!({"type": "object"}),
            ToolOutputMode::Text,
            ToolOrigin::Mcp {
                server_name: "fixture".into(),
            },
            ToolSource::McpTool {
                server_name: "fixture".into(),
            },
        )
        .with_mcp_boundary(McpToolBoundary::local_process(McpTransportKind::Stdio));

        assert_eq!(
            spec.effective_mcp_boundary(&call),
            Some(&McpToolBoundary::local_process(McpTransportKind::Stdio))
        );
    }

    #[test]
    fn shared_mcp_boundary_is_resolved_from_server_name_argument() {
        let call = ToolCall {
            id: ToolCallId::new(),
            call_id: "call-2".into(),
            tool_name: "read_mcp_resource".into(),
            arguments: json!({"server_name": "fixture", "uri": "fixture://guide"}),
            origin: ToolOrigin::Mcp {
                server_name: "*".into(),
            },
        };
        let spec = ToolSpec::function(
            "read_mcp_resource",
            "read",
            json!({"type": "object"}),
            ToolOutputMode::ContentParts,
            ToolOrigin::Mcp {
                server_name: "*".into(),
            },
            ToolSource::McpResource {
                server_name: "*".into(),
            },
        )
        .with_mcp_server_boundaries(BTreeMap::from([(
            "fixture".into(),
            McpToolBoundary::remote_service(McpTransportKind::StreamableHttp),
        )]));

        assert_eq!(
            spec.effective_mcp_boundary(&call),
            Some(&McpToolBoundary::remote_service(
                McpTransportKind::StreamableHttp,
            ))
        );
    }

    #[test]
    fn visibility_context_honors_provider_model_and_role_allowlists() {
        let spec = ToolSpec::function(
            "patch_files",
            "apply patch files",
            json!({"type": "object"}),
            ToolOutputMode::Text,
            ToolOrigin::Local,
            ToolSource::Builtin,
        )
        .with_availability(ToolAvailability {
            provider_allowlist: vec!["openai".to_string()],
            model_allowlist: vec!["gpt-5*".to_string()],
            role_allowlist: vec!["worker".to_string()],
            ..ToolAvailability::default()
        });

        assert!(
            spec.is_model_visible(
                &ToolVisibilityContext::default()
                    .with_provider("openai")
                    .with_model("gpt-5.4")
                    .with_role("worker")
            )
        );
        assert!(
            !spec.is_model_visible(
                &ToolVisibilityContext::default()
                    .with_provider("openai")
                    .with_model("gpt-4.1-mini")
                    .with_role("worker")
            )
        );
        assert!(
            !spec.is_model_visible(
                &ToolVisibilityContext::default()
                    .with_provider("anthropic")
                    .with_model("claude-sonnet-4-6")
                    .with_role("worker")
            )
        );
        assert!(
            !spec.is_model_visible(
                &ToolVisibilityContext::default()
                    .with_provider("openai")
                    .with_model("gpt-5.4")
            )
        );
    }

    #[test]
    fn visibility_context_requires_declared_feature_flags() {
        let spec = ToolSpec::function(
            "request_permissions",
            "request permissions",
            json!({"type": "object"}),
            ToolOutputMode::Text,
            ToolOrigin::Local,
            ToolSource::Builtin,
        )
        .with_availability(ToolAvailability {
            feature_flags: vec![
                "host-user-input".to_string(),
                "host-permission-request".to_string(),
            ],
            ..ToolAvailability::default()
        });

        assert!(!spec.is_model_visible(&ToolVisibilityContext::default()));
        assert!(
            !spec.is_model_visible(
                &ToolVisibilityContext::default().with_feature("host-user-input")
            )
        );
        assert!(
            spec.is_model_visible(
                &ToolVisibilityContext::default()
                    .with_feature("host-user-input")
                    .with_feature("host-permission-request")
            )
        );
    }

    #[test]
    fn unknown_provider_and_model_keep_legacy_visibility_behavior() {
        let spec = ToolSpec::function(
            "patch_files",
            "apply patch files",
            json!({"type": "object"}),
            ToolOutputMode::Text,
            ToolOrigin::Local,
            ToolSource::Builtin,
        )
        .with_availability(ToolAvailability {
            provider_allowlist: vec!["openai".to_string()],
            model_allowlist: vec!["gpt-5*".to_string()],
            ..ToolAvailability::default()
        });

        assert!(spec.is_model_visible(&ToolVisibilityContext::default()));
        assert!(spec.is_model_visible(&ToolVisibilityContext::default().with_provider("unknown")));
    }

    #[test]
    fn function_tools_can_offer_provider_scoped_freeform_transport() {
        let spec = ToolSpec::function(
            "patch_files",
            "apply patch files",
            json!({"type": "object"}),
            ToolOutputMode::Text,
            ToolOrigin::Local,
            ToolSource::Builtin,
        )
        .with_freeform_format(ToolFreeformFormat::grammar("lark", "start: patch"))
        .with_freeform_availability(ToolAvailability {
            provider_allowlist: vec!["openai".to_string()],
            model_allowlist: vec!["gpt-5*".to_string()],
            ..ToolAvailability::default()
        });

        assert!(
            spec.supports_freeform_transport(
                &ToolVisibilityContext::default()
                    .with_provider("openai")
                    .with_model("gpt-5.4")
            )
        );
        assert!(
            !spec.supports_freeform_transport(
                &ToolVisibilityContext::default()
                    .with_provider("openai")
                    .with_model("gpt-4.1-mini")
            )
        );
        assert!(!spec.supports_freeform_transport(
            &ToolVisibilityContext::default().with_provider("anthropic")
        ));
    }
}
