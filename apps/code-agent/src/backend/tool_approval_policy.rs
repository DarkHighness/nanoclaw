use agent::runtime::{ToolApprovalPolicy, ToolApprovalPolicyDecision, ToolApprovalRequest};
use agent::types::{ToolName, ToolOrigin, ToolSource};
use std::collections::BTreeSet;

const AUTO_ALLOW_LOCAL_READ_ONLY_TOOL_NAMES: [&str; 2] = ["web_search", "web_fetch"];

#[derive(Clone, Debug, Default)]
pub(crate) struct CodeAgentToolApprovalPolicy {
    auto_allow_tool_names: BTreeSet<ToolName>,
}

/// Code Agent keeps approval relaxation host-scoped on purpose.
///
/// The runtime baseline still treats network and open-world access as approval
/// worthy because that is the safer generic default across hosts. This helper
/// only suppresses those baseline reasons for a tiny code-agent-owned allowlist
/// of built-in read-only research tools instead of trusting arbitrary
/// `read_only` metadata from MCP or custom tools. The policy also checks the
/// resolved tool source so these names stay bound to built-in implementations
/// instead of silently widening trust to a future dynamic tool.
pub(crate) fn build_code_agent_tool_approval_policy() -> CodeAgentToolApprovalPolicy {
    CodeAgentToolApprovalPolicy {
        auto_allow_tool_names: auto_allow_tool_names(),
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
        if request.call.origin != ToolOrigin::Local {
            return ToolApprovalPolicyDecision::Abstain;
        }
        if request.spec.source != ToolSource::Builtin {
            return ToolApprovalPolicyDecision::Abstain;
        }
        if self.auto_allow_tool_names.contains(&request.call.tool_name) {
            return ToolApprovalPolicyDecision::Allow;
        }
        ToolApprovalPolicyDecision::Abstain
    }
}

#[cfg(test)]
mod tests {
    use super::build_code_agent_tool_approval_policy;
    use agent::runtime::{ToolApprovalPolicy, ToolApprovalPolicyDecision};
    use agent::types::{ToolCall, ToolCallId, ToolOrigin, ToolOutputMode, ToolSource, ToolSpec};
    use serde_json::json;

    fn request(
        tool_name: &str,
        origin: ToolOrigin,
        source: ToolSource,
    ) -> agent::runtime::ToolApprovalRequest {
        agent::runtime::ToolApprovalRequest {
            call: ToolCall {
                id: ToolCallId::new(),
                call_id: format!("call-{tool_name}").into(),
                tool_name: tool_name.into(),
                arguments: json!({}),
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

    #[test]
    fn auto_allows_builtin_local_web_tools() {
        let policy = build_code_agent_tool_approval_policy();

        assert_eq!(
            policy.decide(&request(
                "web_search",
                ToolOrigin::Local,
                ToolSource::Builtin
            )),
            ToolApprovalPolicyDecision::Allow
        );
        assert_eq!(
            policy.decide(&request(
                "web_fetch",
                ToolOrigin::Local,
                ToolSource::Builtin
            )),
            ToolApprovalPolicyDecision::Allow
        );
    }

    #[test]
    fn keeps_other_or_non_local_reads_on_the_default_path() {
        let policy = build_code_agent_tool_approval_policy();

        assert_eq!(
            policy.decide(&request(
                "read_mcp_resource",
                ToolOrigin::Local,
                ToolSource::Builtin
            )),
            ToolApprovalPolicyDecision::Abstain
        );
        assert_eq!(
            policy.decide(&request(
                "web_search",
                ToolOrigin::Mcp {
                    server_name: "docs".into()
                },
                ToolSource::Builtin
            )),
            ToolApprovalPolicyDecision::Abstain
        );
    }

    #[test]
    fn keeps_name_based_allowlist_bound_to_builtin_sources() {
        let policy = build_code_agent_tool_approval_policy();

        assert_eq!(
            policy.decide(&request(
                "web_search",
                ToolOrigin::Local,
                ToolSource::Dynamic
            )),
            ToolApprovalPolicyDecision::Abstain
        );
    }
}
