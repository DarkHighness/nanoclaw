use crate::Result;
use regex::Regex;
use types::{HookContext, HookRegistration};

pub fn matches_hook(registration: &HookRegistration, context: &HookContext) -> Result<bool> {
    let Some(matcher) = &registration.matcher else {
        return Ok(true);
    };
    let regex = Regex::new(&matcher.pattern)?;
    let value = matcher
        .field
        .as_deref()
        .or_else(|| context.event.default_match_field())
        .and_then(|field| context.field(field).map(ToOwned::to_owned))
        .unwrap_or_else(|| context.payload.to_string());
    Ok(regex.is_match(&value))
}

#[cfg(test)]
mod tests {
    use super::matches_hook;
    use types::{
        AgentSessionId, CommandHookHandler, HookContext, HookEvent, HookHandler, HookMatcher,
        HookRegistration, SessionId,
    };

    #[test]
    fn matches_tool_name_by_default_field() {
        let registration = HookRegistration {
            name: "tool".to_string(),
            event: HookEvent::PreToolUse,
            matcher: Some(HookMatcher {
                pattern: "^read$".to_string(),
                field: None,
            }),
            handler: HookHandler::Command(CommandHookHandler {
                command: "true".to_string(),
                asynchronous: false,
            }),
            timeout_ms: None,
            execution: None,
        };
        let context = HookContext {
            event: HookEvent::PreToolUse,
            session_id: SessionId::new(),
            agent_session_id: AgentSessionId::new(),
            turn_id: None,
            fields: [("tool_name".to_string(), "read".to_string())]
                .into_iter()
                .collect(),
            payload: serde_json::json!({}),
        };
        assert!(matches_hook(&registration, &context).unwrap());
    }
}
