use agent::runtime::{ToolApprovalPolicy, ToolApprovalPolicyDecision, ToolApprovalRequest};
use agent::types::{McpToolBoundaryClass, ToolName, ToolOrigin, ToolSource};
use serde_json::Value;
use std::collections::BTreeSet;

const AUTO_ALLOW_LOCAL_READ_ONLY_TOOL_NAMES: [&str; 2] = ["web_search", "web_fetch"];

#[derive(Clone, Debug, Default)]
pub(crate) struct CodeAgentToolApprovalPolicy {
    auto_allow_tool_names: BTreeSet<ToolName>,
    exec_always_approve_simple_prefixes: Vec<String>,
}

/// Code Agent keeps approval relaxation host-scoped on purpose.
///
/// The runtime baseline still treats network and open-world access as approval
/// worthy because that is the safer generic default across hosts. This helper
/// only suppresses those baseline reasons for a tiny code-agent-owned allowlist
/// of built-in read-only research tools, local-process MCP resource reads, and
/// configured simple exec prefixes instead of trusting arbitrary `read_only`
/// metadata from MCP or custom tools. The policy also checks the
/// resolved tool source so these names stay bound to built-in implementations
/// instead of silently widening trust to a future dynamic tool.
pub(crate) fn build_code_agent_tool_approval_policy(
    exec_always_approve_simple_prefixes: &[String],
) -> CodeAgentToolApprovalPolicy {
    CodeAgentToolApprovalPolicy {
        auto_allow_tool_names: auto_allow_tool_names(),
        exec_always_approve_simple_prefixes: exec_always_approve_simple_prefixes.to_vec(),
    }
}

fn auto_allow_tool_names() -> BTreeSet<ToolName> {
    AUTO_ALLOW_LOCAL_READ_ONLY_TOOL_NAMES
        .into_iter()
        .map(ToolName::from)
        .collect()
}

impl ToolApprovalPolicy for CodeAgentToolApprovalPolicy {
    fn decide(&self, request: &ToolApprovalRequest) -> ToolApprovalPolicyDecision {
        if is_auto_allowed_local_mcp_resource_read(request) {
            return ToolApprovalPolicyDecision::Allow;
        }
        if !is_builtin_local_request(request) {
            return ToolApprovalPolicyDecision::Abstain;
        }
        if self.auto_allow_tool_names.contains(&request.call.tool_name) {
            return ToolApprovalPolicyDecision::Allow;
        }
        if request.call.tool_name.as_str() == "exec_command"
            && is_auto_approved_simple_exec_command(
                &request.call.arguments,
                &self.exec_always_approve_simple_prefixes,
            )
        {
            return ToolApprovalPolicyDecision::Allow;
        }
        ToolApprovalPolicyDecision::Abstain
    }
}

fn is_builtin_local_request(request: &ToolApprovalRequest) -> bool {
    request.call.origin == ToolOrigin::Local && request.spec.source == ToolSource::Builtin
}

fn is_auto_allowed_local_mcp_resource_read(request: &ToolApprovalRequest) -> bool {
    if request.call.tool_name.as_str() != "read_mcp_resource" {
        return false;
    }
    if !matches!(request.spec.source, ToolSource::McpResource { .. }) {
        return false;
    }
    request
        .spec
        .effective_mcp_boundary(&request.call)
        .is_some_and(|boundary| boundary.boundary_class == McpToolBoundaryClass::LocalProcess)
}

fn is_auto_approved_simple_exec_command(arguments: &Value, prefixes: &[String]) -> bool {
    let Some(command) = arguments
        .get("cmd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    if !is_simple_shell_command(command) {
        return false;
    }
    prefixes
        .iter()
        .any(|prefix| matches_simple_command_prefix(command, prefix))
}

fn matches_simple_command_prefix(command: &str, prefix: &str) -> bool {
    let prefix = prefix.trim();
    if prefix.is_empty() || !is_simple_shell_command(prefix) {
        return false;
    }
    if command == prefix {
        return true;
    }
    command
        .strip_prefix(prefix)
        .and_then(|rest| rest.chars().next())
        .is_some_and(char::is_whitespace)
}

fn is_simple_shell_command(command: &str) -> bool {
    // exec_command always runs through the shell, so host-scoped auto-allow
    // rules intentionally fail closed on shell control syntax. This keeps the
    // config limited to simple single-command prefixes instead of silently
    // trusting pipelines, redirects, substitutions, or chained commands.
    !command
        .chars()
        .any(|ch| matches!(ch, ';' | '&' | '|' | '>' | '<' | '`' | '$' | '\n' | '\r'))
}

#[cfg(test)]
mod tests {
    use super::build_code_agent_tool_approval_policy;
    use agent::runtime::{ToolApprovalPolicy, ToolApprovalPolicyDecision};
    use agent::types::{
        McpToolBoundary, McpTransportKind, ToolCall, ToolCallId, ToolOrigin, ToolOutputMode,
        ToolSource, ToolSpec,
    };
    use serde_json::json;
    use std::collections::BTreeMap;

    fn request(
        tool_name: &str,
        origin: ToolOrigin,
        source: ToolSource,
        arguments: serde_json::Value,
    ) -> agent::runtime::ToolApprovalRequest {
        agent::runtime::ToolApprovalRequest {
            call: ToolCall {
                id: ToolCallId::new(),
                call_id: format!("call-{tool_name}").into(),
                tool_name: tool_name.into(),
                arguments,
                origin: origin.clone(),
            },
            spec: ToolSpec::function(
                tool_name,
                tool_name,
                json!({"type":"object"}),
                ToolOutputMode::Text,
                origin,
                source,
            ),
            reasons: Vec::new(),
        }
    }

    fn mcp_resource_request(
        server_name: &str,
        boundary: McpToolBoundary,
    ) -> agent::runtime::ToolApprovalRequest {
        agent::runtime::ToolApprovalRequest {
            call: ToolCall {
                id: ToolCallId::new(),
                call_id: format!("call-read-{server_name}").into(),
                tool_name: "read_mcp_resource".into(),
                arguments: json!({"server_name": server_name, "uri": format!("{server_name}://guide")}),
                origin: ToolOrigin::Mcp {
                    server_name: "*".into(),
                },
            },
            spec: ToolSpec::function(
                "read_mcp_resource",
                "read_mcp_resource",
                json!({"type":"object"}),
                ToolOutputMode::ContentParts,
                ToolOrigin::Mcp {
                    server_name: "*".into(),
                },
                ToolSource::McpResource {
                    server_name: "*".into(),
                },
            )
            .with_mcp_server_boundaries(BTreeMap::from([(server_name.into(), boundary)])),
            reasons: Vec::new(),
        }
    }

    #[test]
    fn auto_allows_builtin_local_web_tools() {
        let policy = build_code_agent_tool_approval_policy(&[]);

        assert_eq!(
            policy.decide(&request(
                "web_search",
                ToolOrigin::Local,
                ToolSource::Builtin,
                json!({})
            )),
            ToolApprovalPolicyDecision::Allow
        );
        assert_eq!(
            policy.decide(&request(
                "web_fetch",
                ToolOrigin::Local,
                ToolSource::Builtin,
                json!({})
            )),
            ToolApprovalPolicyDecision::Allow
        );
    }

    #[test]
    fn keeps_other_or_non_local_reads_on_the_default_path() {
        let policy = build_code_agent_tool_approval_policy(&[]);

        assert_eq!(
            policy.decide(&request(
                "read_mcp_resource",
                ToolOrigin::Local,
                ToolSource::Builtin,
                json!({})
            )),
            ToolApprovalPolicyDecision::Abstain
        );
        assert_eq!(
            policy.decide(&request(
                "web_search",
                ToolOrigin::Mcp {
                    server_name: "docs".into()
                },
                ToolSource::Builtin,
                json!({})
            )),
            ToolApprovalPolicyDecision::Abstain
        );
    }

    #[test]
    fn auto_allows_local_process_mcp_resource_reads_only() {
        let policy = build_code_agent_tool_approval_policy(&[]);

        assert_eq!(
            policy.decide(&mcp_resource_request(
                "fixture",
                McpToolBoundary::local_process(McpTransportKind::Stdio)
            )),
            ToolApprovalPolicyDecision::Allow
        );
        assert_eq!(
            policy.decide(&mcp_resource_request(
                "docs",
                McpToolBoundary::remote_service(McpTransportKind::StreamableHttp)
            )),
            ToolApprovalPolicyDecision::Abstain
        );
    }

    #[test]
    fn keeps_name_based_allowlist_bound_to_builtin_sources() {
        let policy = build_code_agent_tool_approval_policy(&[]);

        assert_eq!(
            policy.decide(&request(
                "web_search",
                ToolOrigin::Local,
                ToolSource::Dynamic,
                json!({})
            )),
            ToolApprovalPolicyDecision::Abstain
        );
    }

    #[test]
    fn auto_allows_configured_simple_exec_prefixes() {
        let policy = build_code_agent_tool_approval_policy(&["git status".to_string()]);

        assert_eq!(
            policy.decide(&request(
                "exec_command",
                ToolOrigin::Local,
                ToolSource::Builtin,
                json!({"cmd": "git status --short"})
            )),
            ToolApprovalPolicyDecision::Allow
        );
    }

    #[test]
    fn keeps_shell_control_syntax_on_the_default_approval_path() {
        let policy = build_code_agent_tool_approval_policy(&["git status".to_string()]);

        assert_eq!(
            policy.decide(&request(
                "exec_command",
                ToolOrigin::Local,
                ToolSource::Builtin,
                json!({"cmd": "git status; rm -rf ."})
            )),
            ToolApprovalPolicyDecision::Abstain
        );
    }

    #[test]
    fn keeps_write_stdin_on_the_default_path() {
        let policy = build_code_agent_tool_approval_policy(&[]);

        assert_eq!(
            policy.decide(&request(
                "write_stdin",
                ToolOrigin::Local,
                ToolSource::Builtin,
                json!({"session_id": "exec-123"})
            )),
            ToolApprovalPolicyDecision::Abstain
        );
        assert_eq!(
            policy.decide(&request(
                "write_stdin",
                ToolOrigin::Local,
                ToolSource::Builtin,
                json!({"session_id": "exec-123", "chars": "y\n"})
            )),
            ToolApprovalPolicyDecision::Abstain
        );
        assert_eq!(
            policy.decide(&request(
                "write_stdin",
                ToolOrigin::Local,
                ToolSource::Builtin,
                json!({"session_id": "exec-123", "close_stdin": true})
            )),
            ToolApprovalPolicyDecision::Abstain
        );
    }
}
