use crate::Result;
use async_trait::async_trait;
use reqwest::Url;
use serde_json::Value;
use std::collections::BTreeSet;
use types::{ToolCall, ToolSpec};

#[derive(Clone, Debug, PartialEq)]
pub struct ToolApprovalRequest {
    pub call: ToolCall,
    pub spec: ToolSpec,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolApprovalOutcome {
    Approve,
    Deny { reason: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolApprovalPolicyDecision {
    Allow,
    Ask { reason: Option<String> },
    Deny { reason: Option<String> },
    Abstain,
}

pub trait ToolApprovalPolicy: Send + Sync {
    fn decide(&self, request: &ToolApprovalRequest) -> ToolApprovalPolicyDecision;
}

#[async_trait]
pub trait ToolApprovalHandler: Send + Sync {
    async fn decide(&self, request: ToolApprovalRequest) -> Result<ToolApprovalOutcome>;
}

#[derive(Default)]
pub struct NoopToolApprovalPolicy;

impl ToolApprovalPolicy for NoopToolApprovalPolicy {
    fn decide(&self, _request: &ToolApprovalRequest) -> ToolApprovalPolicyDecision {
        ToolApprovalPolicyDecision::Abstain
    }
}

#[derive(Default)]
pub struct AlwaysAllowToolApprovalHandler;

#[async_trait]
impl ToolApprovalHandler for AlwaysAllowToolApprovalHandler {
    async fn decide(&self, _request: ToolApprovalRequest) -> Result<ToolApprovalOutcome> {
        Ok(ToolApprovalOutcome::Approve)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolApprovalRuleEffect {
    Allow,
    Ask,
    Deny,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolOriginMatcher {
    Local,
    McpServer { server_name: String },
    Provider { provider: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StringMatcher {
    Exact(String),
    Prefix(String),
    Suffix(String),
    Contains(String),
}

impl StringMatcher {
    fn matches(&self, candidate: &str) -> bool {
        match self {
            Self::Exact(expected) => candidate == expected,
            Self::Prefix(prefix) => candidate.starts_with(prefix),
            Self::Suffix(suffix) => candidate.ends_with(suffix),
            Self::Contains(fragment) => candidate.contains(fragment),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolArgumentMatcher {
    Exists {
        pointer: String,
    },
    ValueEquals {
        pointer: String,
        value: Value,
    },
    String {
        pointer: String,
        matcher: StringMatcher,
    },
    UrlHost {
        pointer: String,
        matcher: StringMatcher,
    },
}

impl ToolArgumentMatcher {
    fn matches(&self, arguments: &Value) -> bool {
        match self {
            Self::Exists { pointer } => arguments.pointer(pointer).is_some(),
            Self::ValueEquals { pointer, value } => arguments.pointer(pointer) == Some(value),
            Self::String { pointer, matcher } => arguments
                .pointer(pointer)
                .and_then(Value::as_str)
                .is_some_and(|candidate| matcher.matches(candidate)),
            Self::UrlHost { pointer, matcher } => arguments
                .pointer(pointer)
                .and_then(Value::as_str)
                .and_then(|candidate| Url::parse(candidate).ok())
                .and_then(|url| url.host_str().map(ToOwned::to_owned))
                .is_some_and(|host| matcher.matches(&host)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ToolApprovalMatcher {
    pub tool_names: BTreeSet<String>,
    pub origins: Vec<ToolOriginMatcher>,
    pub argument_matchers: Vec<ToolArgumentMatcher>,
}

impl ToolApprovalMatcher {
    #[must_use]
    pub fn matches(&self, request: &ToolApprovalRequest) -> bool {
        if !self.tool_names.is_empty() && !self.tool_names.contains(&request.call.tool_name) {
            return false;
        }
        if !self.origins.is_empty()
            && !self
                .origins
                .iter()
                .any(|origin| origin_matches(origin, &request.call.origin))
        {
            return false;
        }
        self.argument_matchers
            .iter()
            .all(|matcher| matcher.matches(&request.call.arguments))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolApprovalRule {
    pub matcher: ToolApprovalMatcher,
    pub effect: ToolApprovalRuleEffect,
    pub reason: Option<String>,
}

impl ToolApprovalRule {
    #[must_use]
    pub fn allow(matcher: ToolApprovalMatcher, reason: impl Into<String>) -> Self {
        Self {
            matcher,
            effect: ToolApprovalRuleEffect::Allow,
            reason: Some(reason.into()),
        }
    }

    #[must_use]
    pub fn ask(matcher: ToolApprovalMatcher, reason: impl Into<String>) -> Self {
        Self {
            matcher,
            effect: ToolApprovalRuleEffect::Ask,
            reason: Some(reason.into()),
        }
    }

    #[must_use]
    pub fn deny(matcher: ToolApprovalMatcher, reason: impl Into<String>) -> Self {
        Self {
            matcher,
            effect: ToolApprovalRuleEffect::Deny,
            reason: Some(reason.into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ToolApprovalRuleSet {
    rules: Vec<ToolApprovalRule>,
}

impl ToolApprovalRuleSet {
    #[must_use]
    pub fn new(rules: Vec<ToolApprovalRule>) -> Self {
        Self { rules }
    }

    pub fn push(&mut self, rule: ToolApprovalRule) {
        self.rules.push(rule);
    }

    #[must_use]
    pub fn rules(&self) -> &[ToolApprovalRule] {
        &self.rules
    }
}

impl ToolApprovalPolicy for ToolApprovalRuleSet {
    fn decide(&self, request: &ToolApprovalRequest) -> ToolApprovalPolicyDecision {
        // Approval rules are first-match wins so hosts can express a narrow
        // allowlist before a broader denylist or "ask" default.
        for rule in &self.rules {
            if !rule.matcher.matches(request) {
                continue;
            }
            return match rule.effect {
                ToolApprovalRuleEffect::Allow => ToolApprovalPolicyDecision::Allow,
                ToolApprovalRuleEffect::Ask => ToolApprovalPolicyDecision::Ask {
                    reason: rule.reason.clone(),
                },
                ToolApprovalRuleEffect::Deny => ToolApprovalPolicyDecision::Deny {
                    reason: rule.reason.clone(),
                },
            };
        }
        ToolApprovalPolicyDecision::Abstain
    }
}

fn origin_matches(matcher: &ToolOriginMatcher, origin: &types::ToolOrigin) -> bool {
    match (matcher, origin) {
        (ToolOriginMatcher::Local, types::ToolOrigin::Local) => true,
        (
            ToolOriginMatcher::McpServer { server_name: left },
            types::ToolOrigin::Mcp { server_name: right },
        ) => left == right,
        (
            ToolOriginMatcher::Provider { provider: left },
            types::ToolOrigin::Provider { provider: right },
        ) => left == right,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        StringMatcher, ToolApprovalMatcher, ToolApprovalPolicy, ToolApprovalPolicyDecision,
        ToolApprovalRequest, ToolApprovalRule, ToolApprovalRuleSet, ToolArgumentMatcher,
        ToolOriginMatcher,
    };
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};
    use types::{ToolCall, ToolCallId, ToolOrigin, ToolOutputMode, ToolSpec};

    fn request(arguments: serde_json::Value) -> ToolApprovalRequest {
        ToolApprovalRequest {
            call: ToolCall {
                id: ToolCallId::new(),
                call_id: "call_1".into(),
                tool_name: "bash".to_string(),
                arguments,
                origin: ToolOrigin::Local,
            },
            spec: ToolSpec {
                name: "bash".to_string(),
                description: "Run commands".to_string(),
                input_schema: json!({"type":"object"}),
                output_mode: ToolOutputMode::Text,
                origin: ToolOrigin::Local,
                annotations: BTreeMap::new(),
            },
            reasons: Vec::new(),
        }
    }

    #[test]
    fn rule_set_matches_tool_origin_and_argument_prefix() {
        let mut tool_names = BTreeSet::new();
        tool_names.insert("bash".to_string());
        let rules = ToolApprovalRuleSet::new(vec![ToolApprovalRule::ask(
            ToolApprovalMatcher {
                tool_names,
                origins: vec![ToolOriginMatcher::Local],
                argument_matchers: vec![ToolArgumentMatcher::String {
                    pointer: "/command".to_string(),
                    matcher: StringMatcher::Prefix("git ".to_string()),
                }],
            },
            "review git invocations",
        )]);

        let decision = rules.decide(&request(json!({"command":"git status"})));

        assert_eq!(
            decision,
            ToolApprovalPolicyDecision::Ask {
                reason: Some("review git invocations".to_string())
            }
        );
    }

    #[test]
    fn rule_set_can_match_url_hosts() {
        let rules = ToolApprovalRuleSet::new(vec![ToolApprovalRule::deny(
            ToolApprovalMatcher {
                tool_names: ["web_fetch".to_string()].into_iter().collect(),
                origins: vec![ToolOriginMatcher::Local],
                argument_matchers: vec![ToolArgumentMatcher::UrlHost {
                    pointer: "/url".to_string(),
                    matcher: StringMatcher::Suffix(".internal".to_string()),
                }],
            },
            "blocked internal host",
        )]);
        let mut request = request(json!({"url":"https://ops.example.internal/health"}));
        request.call.tool_name = "web_fetch".to_string();
        request.spec.name = "web_fetch".to_string();

        let decision = rules.decide(&request);

        assert_eq!(
            decision,
            ToolApprovalPolicyDecision::Deny {
                reason: Some("blocked internal host".to_string())
            }
        );
    }

    #[test]
    fn rules_are_first_match_wins() {
        let rules = ToolApprovalRuleSet::new(vec![
            ToolApprovalRule::allow(
                ToolApprovalMatcher {
                    tool_names: ["bash".to_string()].into_iter().collect(),
                    origins: vec![ToolOriginMatcher::Local],
                    argument_matchers: vec![ToolArgumentMatcher::String {
                        pointer: "/command".to_string(),
                        matcher: StringMatcher::Prefix("git status".to_string()),
                    }],
                },
                "status is safe",
            ),
            ToolApprovalRule::deny(
                ToolApprovalMatcher {
                    tool_names: ["bash".to_string()].into_iter().collect(),
                    origins: vec![ToolOriginMatcher::Local],
                    argument_matchers: vec![ToolArgumentMatcher::Exists {
                        pointer: "/command".to_string(),
                    }],
                },
                "all other bash commands denied",
            ),
        ]);

        let decision = rules.decide(&request(json!({"command":"git status --short"})));

        assert_eq!(decision, ToolApprovalPolicyDecision::Allow);
    }
}
