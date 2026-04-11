use crate::config::{
    CodeAgentApprovalMcpBoundaryMatcher, CodeAgentApprovalOriginMatcher,
    CodeAgentApprovalPolicyConfig, CodeAgentApprovalRule, CodeAgentApprovalRuleEffect,
    CodeAgentApprovalSourceMatcher, ExecApprovalRule,
};
use agent::runtime::{
    ToolApprovalMatcher, ToolApprovalPolicy, ToolApprovalPolicyDecision, ToolApprovalRequest,
    ToolApprovalRule, ToolApprovalRuleSet, ToolArgumentMatcher, ToolMcpBoundaryMatcher,
    ToolOriginMatcher, ToolSourceMatcher,
};
use agent::types::ToolName;

#[derive(Clone, Debug, Default)]
pub(crate) struct CodeAgentToolApprovalPolicy {
    approval_rules: ToolApprovalRuleSet,
    default_mode: Option<CodeAgentApprovalRuleEffect>,
}

/// Code Agent keeps approval relaxation host-scoped on purpose.
///
/// The runtime baseline still treats network and open-world access as approval
/// worthy because that is the safer generic default across hosts. This helper
/// compiles ordered host approval rules into shared runtime matcher primitives.
pub(crate) fn build_code_agent_tool_approval_policy(
    config: &CodeAgentApprovalPolicyConfig,
) -> CodeAgentToolApprovalPolicy {
    CodeAgentToolApprovalPolicy {
        approval_rules: compile_approval_rules(&config.rules),
        default_mode: config.default_mode.clone(),
    }
}

impl ToolApprovalPolicy for CodeAgentToolApprovalPolicy {
    fn decide(&self, request: &ToolApprovalRequest) -> ToolApprovalPolicyDecision {
        let shared_decision = self.approval_rules.decide(request);
        if shared_decision != ToolApprovalPolicyDecision::Abstain {
            return shared_decision;
        }
        match self.default_mode {
            Some(CodeAgentApprovalRuleEffect::Allow) => ToolApprovalPolicyDecision::Allow,
            Some(CodeAgentApprovalRuleEffect::Ask) => ToolApprovalPolicyDecision::Ask {
                reason: Some("code-agent approval.default_mode requested review".to_string()),
            },
            Some(CodeAgentApprovalRuleEffect::Deny) => ToolApprovalPolicyDecision::Deny {
                reason: Some("code-agent approval.default_mode denied the request".to_string()),
            },
            None => ToolApprovalPolicyDecision::Abstain,
        }
    }
}

fn compile_approval_rules(rules: &[CodeAgentApprovalRule]) -> ToolApprovalRuleSet {
    ToolApprovalRuleSet::new(rules.iter().map(compile_approval_rule).collect())
}

fn compile_approval_rule(rule: &CodeAgentApprovalRule) -> ToolApprovalRule {
    let matcher = ToolApprovalMatcher {
        tool_names: rule
            .tool_names
            .iter()
            .cloned()
            .map(ToolName::from)
            .collect(),
        origins: rule.origins.iter().map(compile_origin_matcher).collect(),
        sources: rule.sources.iter().map(compile_source_matcher).collect(),
        argument_matchers: compile_argument_matchers(rule),
        mcp_boundary: rule.mcp_boundary.as_ref().map(compile_mcp_boundary_matcher),
    };
    match rule.effect {
        CodeAgentApprovalRuleEffect::Allow => ToolApprovalRule::allow(
            matcher,
            rule.reason
                .clone()
                .unwrap_or_else(|| "code-agent approval allow rule".to_string()),
        ),
        CodeAgentApprovalRuleEffect::Ask => ToolApprovalRule::ask(
            matcher,
            rule.reason
                .clone()
                .unwrap_or_else(|| "code-agent approval ask rule".to_string()),
        ),
        CodeAgentApprovalRuleEffect::Deny => ToolApprovalRule::deny(
            matcher,
            rule.reason
                .clone()
                .unwrap_or_else(|| "code-agent approval deny rule".to_string()),
        ),
    }
}

fn compile_origin_matcher(origin: &CodeAgentApprovalOriginMatcher) -> ToolOriginMatcher {
    match origin {
        CodeAgentApprovalOriginMatcher::Local => ToolOriginMatcher::Local,
        CodeAgentApprovalOriginMatcher::McpServer(server_name) => ToolOriginMatcher::McpServer {
            server_name: server_name.clone().into(),
        },
        CodeAgentApprovalOriginMatcher::Provider(provider) => ToolOriginMatcher::Provider {
            provider: provider.clone(),
        },
    }
}

fn compile_source_matcher(source: &CodeAgentApprovalSourceMatcher) -> ToolSourceMatcher {
    match source {
        CodeAgentApprovalSourceMatcher::Builtin => ToolSourceMatcher::Builtin,
        CodeAgentApprovalSourceMatcher::Dynamic => ToolSourceMatcher::Dynamic,
        CodeAgentApprovalSourceMatcher::Plugin => ToolSourceMatcher::Plugin,
        CodeAgentApprovalSourceMatcher::McpTool => ToolSourceMatcher::McpTool,
        CodeAgentApprovalSourceMatcher::McpResource => ToolSourceMatcher::McpResource,
        CodeAgentApprovalSourceMatcher::ProviderBuiltin(provider) => {
            ToolSourceMatcher::ProviderBuiltin {
                provider: provider.clone(),
            }
        }
    }
}

fn compile_mcp_boundary_matcher(
    matcher: &CodeAgentApprovalMcpBoundaryMatcher,
) -> ToolMcpBoundaryMatcher {
    ToolMcpBoundaryMatcher {
        transports: matcher.transports.clone(),
        boundary_classes: matcher.boundary_classes.clone(),
    }
}

fn compile_argument_matchers(rule: &CodeAgentApprovalRule) -> Vec<ToolArgumentMatcher> {
    let mut matchers = Vec::new();
    if let Some(exec_rule) = &rule.exec {
        matchers.push(match exec_rule {
            ExecApprovalRule::ArgvExact(argv) => ToolArgumentMatcher::SimpleShellArgvExact {
                pointer: "/cmd".to_string(),
                argv: argv.clone(),
            },
            ExecApprovalRule::ArgvPrefix(argv) => ToolArgumentMatcher::SimpleShellArgvPrefix {
                pointer: "/cmd".to_string(),
                argv: argv.clone(),
            },
        });
    }
    matchers
}

#[cfg(test)]
mod tests {
    use super::build_code_agent_tool_approval_policy;
    use crate::config::{
        CodeAgentApprovalMcpBoundaryMatcher, CodeAgentApprovalOriginMatcher,
        CodeAgentApprovalPolicyConfig, CodeAgentApprovalRule, CodeAgentApprovalRuleEffect,
        CodeAgentApprovalSourceMatcher, ExecApprovalRule,
    };
    use agent::runtime::{ToolApprovalPolicy, ToolApprovalPolicyDecision};
    use agent::types::{
        McpToolBoundary, McpToolBoundaryClass, McpTransportKind, ToolCall, ToolCallId, ToolOrigin,
        ToolOutputMode, ToolSource, ToolSpec,
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

    fn policy_config() -> CodeAgentApprovalPolicyConfig {
        CodeAgentApprovalPolicyConfig {
            default_mode: None,
            rules: vec![
                CodeAgentApprovalRule {
                    effect: CodeAgentApprovalRuleEffect::Allow,
                    reason: Some("code-agent built-in local allowlist".to_string()),
                    tool_names: ["web_search".to_string(), "web_fetch".to_string()]
                        .into_iter()
                        .collect(),
                    origins: vec![CodeAgentApprovalOriginMatcher::Local],
                    sources: vec![CodeAgentApprovalSourceMatcher::Builtin],
                    mcp_boundary: None,
                    exec: None,
                },
                CodeAgentApprovalRule {
                    effect: CodeAgentApprovalRuleEffect::Allow,
                    reason: Some("code-agent local stdio MCP resource allowlist".to_string()),
                    tool_names: ["read_mcp_resource".to_string()].into_iter().collect(),
                    origins: vec![CodeAgentApprovalOriginMatcher::McpServer("*".to_string())],
                    sources: vec![CodeAgentApprovalSourceMatcher::McpResource],
                    mcp_boundary: Some(CodeAgentApprovalMcpBoundaryMatcher {
                        transports: vec![McpTransportKind::Stdio],
                        boundary_classes: vec![McpToolBoundaryClass::LocalProcess],
                    }),
                    exec: None,
                },
            ],
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
        let policy = build_code_agent_tool_approval_policy(&policy_config());

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
        let policy = build_code_agent_tool_approval_policy(&policy_config());

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
        let policy = build_code_agent_tool_approval_policy(&policy_config());

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
    fn keeps_non_stdio_local_process_markings_on_default_path() {
        let policy = build_code_agent_tool_approval_policy(&policy_config());

        assert_eq!(
            policy.decide(&mcp_resource_request(
                "fixture",
                McpToolBoundary {
                    transport: McpTransportKind::StreamableHttp,
                    boundary_class: McpToolBoundaryClass::LocalProcess,
                }
            )),
            ToolApprovalPolicyDecision::Abstain
        );
    }

    #[test]
    fn keeps_name_based_allowlist_bound_to_builtin_sources() {
        let policy = build_code_agent_tool_approval_policy(&policy_config());

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
    fn auto_allows_configured_exec_argv_prefixes() {
        let mut config = policy_config();
        config.rules.push(CodeAgentApprovalRule {
            effect: CodeAgentApprovalRuleEffect::Allow,
            reason: Some("exec argv trust".to_string()),
            tool_names: ["exec_command".to_string()].into_iter().collect(),
            origins: vec![CodeAgentApprovalOriginMatcher::Local],
            sources: vec![CodeAgentApprovalSourceMatcher::Builtin],
            mcp_boundary: None,
            exec: Some(ExecApprovalRule::ArgvPrefix(vec![
                "git".to_string(),
                "status".to_string(),
            ])),
        });
        let policy = build_code_agent_tool_approval_policy(&config);

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
    fn exact_exec_argv_rules_reject_extra_args() {
        let mut config = policy_config();
        config.rules.push(CodeAgentApprovalRule {
            effect: CodeAgentApprovalRuleEffect::Allow,
            reason: Some("exec argv trust".to_string()),
            tool_names: ["exec_command".to_string()].into_iter().collect(),
            origins: vec![CodeAgentApprovalOriginMatcher::Local],
            sources: vec![CodeAgentApprovalSourceMatcher::Builtin],
            mcp_boundary: None,
            exec: Some(ExecApprovalRule::ArgvExact(vec![
                "cargo".to_string(),
                "test".to_string(),
            ])),
        });
        let policy = build_code_agent_tool_approval_policy(&config);

        assert_eq!(
            policy.decide(&request(
                "exec_command",
                ToolOrigin::Local,
                ToolSource::Builtin,
                json!({"cmd": "cargo test"})
            )),
            ToolApprovalPolicyDecision::Allow
        );
        assert_eq!(
            policy.decide(&request(
                "exec_command",
                ToolOrigin::Local,
                ToolSource::Builtin,
                json!({"cmd": "cargo test -p store"})
            )),
            ToolApprovalPolicyDecision::Abstain
        );
    }

    #[test]
    fn quoted_simple_commands_normalize_to_same_argv_match() {
        let mut config = policy_config();
        config.rules.push(CodeAgentApprovalRule {
            effect: CodeAgentApprovalRuleEffect::Allow,
            reason: Some("exec argv trust".to_string()),
            tool_names: ["exec_command".to_string()].into_iter().collect(),
            origins: vec![CodeAgentApprovalOriginMatcher::Local],
            sources: vec![CodeAgentApprovalSourceMatcher::Builtin],
            mcp_boundary: None,
            exec: Some(ExecApprovalRule::ArgvExact(vec![
                "python".to_string(),
                "scripts/check.py".to_string(),
            ])),
        });
        let policy = build_code_agent_tool_approval_policy(&config);

        assert_eq!(
            policy.decide(&request(
                "exec_command",
                ToolOrigin::Local,
                ToolSource::Builtin,
                json!({"cmd": "python 'scripts/check.py'"})
            )),
            ToolApprovalPolicyDecision::Allow
        );
    }

    #[test]
    fn keeps_shell_control_syntax_on_the_default_approval_path() {
        let mut config = policy_config();
        config.rules.push(CodeAgentApprovalRule {
            effect: CodeAgentApprovalRuleEffect::Allow,
            reason: Some("exec argv trust".to_string()),
            tool_names: ["exec_command".to_string()].into_iter().collect(),
            origins: vec![CodeAgentApprovalOriginMatcher::Local],
            sources: vec![CodeAgentApprovalSourceMatcher::Builtin],
            mcp_boundary: None,
            exec: Some(ExecApprovalRule::ArgvPrefix(vec![
                "git".to_string(),
                "status".to_string(),
            ])),
        });
        let policy = build_code_agent_tool_approval_policy(&config);

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
    fn keeps_nested_shell_drivers_on_the_default_path() {
        let mut config = policy_config();
        config.rules.push(CodeAgentApprovalRule {
            effect: CodeAgentApprovalRuleEffect::Allow,
            reason: Some("exec argv trust".to_string()),
            tool_names: ["exec_command".to_string()].into_iter().collect(),
            origins: vec![CodeAgentApprovalOriginMatcher::Local],
            sources: vec![CodeAgentApprovalSourceMatcher::Builtin],
            mcp_boundary: None,
            exec: Some(ExecApprovalRule::ArgvExact(vec![
                "/usr/bin/zsh".to_string(),
                "-lc".to_string(),
                "git status".to_string(),
            ])),
        });
        let policy = build_code_agent_tool_approval_policy(&config);

        assert_eq!(
            policy.decide(&request(
                "exec_command",
                ToolOrigin::Local,
                ToolSource::Builtin,
                json!({"cmd": "/usr/bin/zsh -lc 'git status'"})
            )),
            ToolApprovalPolicyDecision::Abstain
        );
    }

    #[test]
    fn keeps_inline_interpreter_execs_on_the_default_path() {
        let mut config = policy_config();
        config.rules.push(CodeAgentApprovalRule {
            effect: CodeAgentApprovalRuleEffect::Allow,
            reason: Some("exec argv trust".to_string()),
            tool_names: ["exec_command".to_string()].into_iter().collect(),
            origins: vec![CodeAgentApprovalOriginMatcher::Local],
            sources: vec![CodeAgentApprovalSourceMatcher::Builtin],
            mcp_boundary: None,
            exec: Some(ExecApprovalRule::ArgvPrefix(vec!["python".to_string()])),
        });
        let policy = build_code_agent_tool_approval_policy(&config);

        assert_eq!(
            policy.decide(&request(
                "exec_command",
                ToolOrigin::Local,
                ToolSource::Builtin,
                json!({"cmd": "python -c 'print(1)'"})
            )),
            ToolApprovalPolicyDecision::Abstain
        );
    }

    #[test]
    fn keeps_write_stdin_on_the_default_path() {
        let policy = build_code_agent_tool_approval_policy(&policy_config());

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

    #[test]
    fn config_can_disable_local_stdio_mcp_resource_auto_allow() {
        let mut config = policy_config();
        config.rules.retain(|rule| {
            !rule
                .tool_names
                .iter()
                .any(|tool_name| tool_name == "read_mcp_resource")
        });
        let policy = build_code_agent_tool_approval_policy(&config);

        assert_eq!(
            policy.decide(&mcp_resource_request(
                "fixture",
                McpToolBoundary::local_process(McpTransportKind::Stdio)
            )),
            ToolApprovalPolicyDecision::Abstain
        );
    }

    #[test]
    fn config_can_disable_builtin_web_auto_allow() {
        let policy = build_code_agent_tool_approval_policy(&CodeAgentApprovalPolicyConfig {
            default_mode: None,
            rules: vec![CodeAgentApprovalRule {
                effect: CodeAgentApprovalRuleEffect::Allow,
                reason: Some("code-agent local stdio MCP resource allowlist".to_string()),
                tool_names: ["read_mcp_resource".to_string()].into_iter().collect(),
                origins: vec![CodeAgentApprovalOriginMatcher::McpServer("*".to_string())],
                sources: vec![CodeAgentApprovalSourceMatcher::McpResource],
                mcp_boundary: Some(CodeAgentApprovalMcpBoundaryMatcher {
                    transports: vec![McpTransportKind::Stdio],
                    boundary_classes: vec![McpToolBoundaryClass::LocalProcess],
                }),
                exec: None,
            }],
        });

        assert_eq!(
            policy.decide(&request(
                "web_search",
                ToolOrigin::Local,
                ToolSource::Builtin,
                json!({})
            )),
            ToolApprovalPolicyDecision::Abstain
        );
    }

    #[test]
    fn compiled_policy_only_relaxes_builtin_local_tool_names() {
        let policy = build_code_agent_tool_approval_policy(&CodeAgentApprovalPolicyConfig {
            default_mode: None,
            rules: vec![CodeAgentApprovalRule {
                effect: CodeAgentApprovalRuleEffect::Allow,
                reason: Some("code-agent built-in local allowlist".to_string()),
                tool_names: ["web_search".to_string()].into_iter().collect(),
                origins: vec![CodeAgentApprovalOriginMatcher::Local],
                sources: vec![CodeAgentApprovalSourceMatcher::Builtin],
                mcp_boundary: None,
                exec: None,
            }],
        });

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
                "web_search",
                ToolOrigin::Local,
                ToolSource::Dynamic,
                json!({})
            )),
            ToolApprovalPolicyDecision::Abstain
        );
    }

    #[test]
    fn default_mode_can_request_review_for_unmatched_calls() {
        let policy = build_code_agent_tool_approval_policy(&CodeAgentApprovalPolicyConfig {
            default_mode: Some(CodeAgentApprovalRuleEffect::Ask),
            rules: Vec::new(),
        });

        assert_eq!(
            policy.decide(&request(
                "grep",
                ToolOrigin::Local,
                ToolSource::Builtin,
                json!({"pattern": "TODO"})
            )),
            ToolApprovalPolicyDecision::Ask {
                reason: Some("code-agent approval.default_mode requested review".to_string())
            }
        );
    }

    #[test]
    fn deny_rule_can_block_matching_exec_command() {
        let policy = build_code_agent_tool_approval_policy(&CodeAgentApprovalPolicyConfig {
            default_mode: None,
            rules: vec![CodeAgentApprovalRule {
                effect: CodeAgentApprovalRuleEffect::Deny,
                reason: Some("dangerous exec is blocked".to_string()),
                tool_names: ["exec_command".to_string()].into_iter().collect(),
                origins: vec![CodeAgentApprovalOriginMatcher::Local],
                sources: vec![CodeAgentApprovalSourceMatcher::Builtin],
                mcp_boundary: None,
                exec: Some(ExecApprovalRule::ArgvExact(vec![
                    "git".to_string(),
                    "push".to_string(),
                ])),
            }],
        });

        assert_eq!(
            policy.decide(&request(
                "exec_command",
                ToolOrigin::Local,
                ToolSource::Builtin,
                json!({"cmd": "git push"})
            )),
            ToolApprovalPolicyDecision::Deny {
                reason: Some("dangerous exec is blocked".to_string())
            }
        );
    }
}
