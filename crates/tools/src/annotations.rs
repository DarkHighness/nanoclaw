use serde_json::Value;
use types::{ToolApprovalProfile, ToolOrigin, ToolOutputMode, ToolSource, ToolSpec};

/// Maps the current MCP-style tool hints onto the substrate's typed approval
/// profile so local tools share one review contract.
#[must_use]
pub fn tool_approval_profile(
    read_only_hint: bool,
    destructive_hint: bool,
    idempotent_hint: bool,
    open_world_hint: bool,
) -> ToolApprovalProfile {
    ToolApprovalProfile::new(
        read_only_hint,
        destructive_hint,
        Some(idempotent_hint),
        open_world_hint,
    )
}

#[must_use]
pub fn builtin_tool_spec(
    name: &'static str,
    description: impl Into<String>,
    input_schema: Value,
    output_mode: ToolOutputMode,
    approval: ToolApprovalProfile,
) -> ToolSpec {
    ToolSpec::function(
        name,
        description,
        input_schema,
        output_mode,
        ToolOrigin::Local,
        ToolSource::Builtin,
    )
    .with_approval(approval)
}
