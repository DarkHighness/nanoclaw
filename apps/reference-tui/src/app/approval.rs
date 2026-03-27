use anyhow::Result;
use async_trait::async_trait;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::{
    cursor::MoveTo,
    execute,
    terminal::{Clear, ClearType},
};
use runtime::{
    Result as RuntimeResult, RuntimeError, ToolApprovalHandler, ToolApprovalOutcome,
    ToolApprovalRequest,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{self, Stdout, Write};
use std::sync::RwLock;
use types::ToolOrigin;

#[derive(Default)]
pub struct InteractiveToolApprovalHandler {
    session_decisions: RwLock<BTreeMap<ToolApprovalCacheKey, SessionApprovalDecision>>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ToolApprovalCacheKey {
    pub(crate) tool_name: String,
    pub(crate) origin_key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionApprovalDecision {
    Approve,
    Deny,
}

impl ToolApprovalCacheKey {
    pub(crate) fn from_request(request: &ToolApprovalRequest) -> Self {
        Self {
            tool_name: request.call.tool_name.to_string(),
            origin_key: tool_origin_cache_key(&request.call.origin),
        }
    }
}

impl SessionApprovalDecision {
    fn to_outcome(self) -> ToolApprovalOutcome {
        match self {
            SessionApprovalDecision::Approve => ToolApprovalOutcome::Approve,
            SessionApprovalDecision::Deny => ToolApprovalOutcome::Deny {
                reason: Some("tool denied for the rest of the session".to_string()),
            },
        }
    }
}

impl InteractiveToolApprovalHandler {
    pub(crate) fn cached_outcome(
        &self,
        request: &ToolApprovalRequest,
    ) -> Option<ToolApprovalOutcome> {
        self.session_decisions
            .read()
            .unwrap()
            .get(&ToolApprovalCacheKey::from_request(request))
            .copied()
            .map(SessionApprovalDecision::to_outcome)
    }

    pub(crate) fn remember_outcome(
        &self,
        request: &ToolApprovalRequest,
        decision: SessionApprovalDecision,
    ) {
        self.session_decisions
            .write()
            .unwrap()
            .insert(ToolApprovalCacheKey::from_request(request), decision);
    }
}

#[async_trait]
impl ToolApprovalHandler for InteractiveToolApprovalHandler {
    async fn decide(&self, request: ToolApprovalRequest) -> RuntimeResult<ToolApprovalOutcome> {
        if let Some(outcome) = self.cached_outcome(&request) {
            return Ok(outcome);
        }
        let mut stdout = io::stdout();
        loop {
            render_tool_approval_prompt(&mut stdout, &request)
                .map_err(|error| RuntimeError::invalid_state(error.to_string()))?;
            match event::read().map_err(|error| RuntimeError::invalid_state(error.to_string()))? {
                Event::Key(key) => match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        return Ok(ToolApprovalOutcome::Approve);
                    }
                    KeyCode::Char('a') | KeyCode::Char('A') => {
                        self.remember_outcome(&request, SessionApprovalDecision::Approve);
                        return Ok(ToolApprovalOutcome::Approve);
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                        return Ok(ToolApprovalOutcome::Deny {
                            reason: Some("user denied tool call".to_string()),
                        });
                    }
                    KeyCode::Char('d') | KeyCode::Char('D') => {
                        self.remember_outcome(&request, SessionApprovalDecision::Deny);
                        return Ok(ToolApprovalOutcome::Deny {
                            reason: Some("tool denied for the rest of the session".to_string()),
                        });
                    }
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(ToolApprovalOutcome::Deny {
                            reason: Some("user cancelled tool approval".to_string()),
                        });
                    }
                    _ => {}
                },
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
}

pub(super) fn render_tool_approval_prompt(
    stdout: &mut Stdout,
    request: &ToolApprovalRequest,
) -> Result<()> {
    let title = request
        .spec
        .annotations
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or(request.spec.name.as_str());
    let origin = match &request.spec.origin {
        ToolOrigin::Local => "local".to_string(),
        ToolOrigin::Mcp { server_name } => format!("mcp:{server_name}"),
        ToolOrigin::Provider { provider } => format!("provider:{provider}"),
    };
    let arguments = serde_json::to_string_pretty(&request.call.arguments)
        .unwrap_or_else(|_| request.call.arguments.to_string());
    let mut lines = vec![
        "Tool Approval Required".to_string(),
        String::new(),
        format!("tool: {title} ({})", request.call.tool_name),
        format!("origin: {origin}"),
    ];
    if !request.reasons.is_empty() {
        lines.push("reasons:".to_string());
        lines.extend(request.reasons.iter().map(|reason| format!("- {reason}")));
    }
    lines.push("arguments:".to_string());
    lines.extend(truncate_preview(&arguments, 18, 120));
    lines.push(String::new());
    lines.push(
        "y = allow once    a = allow this tool for session    n / Esc = deny once".to_string(),
    );
    lines.push("d = deny this tool for session    Ctrl+C = deny once".to_string());

    execute!(stdout, Clear(ClearType::All), MoveTo(0, 0))?;
    write!(stdout, "{}\r\n", lines.join("\r\n"))?;
    stdout.flush()?;
    Ok(())
}

fn tool_origin_cache_key(origin: &ToolOrigin) -> String {
    match origin {
        ToolOrigin::Local => "local".to_string(),
        ToolOrigin::Mcp { server_name } => format!("mcp:{server_name}"),
        ToolOrigin::Provider { provider } => format!("provider:{provider}"),
    }
}

fn truncate_preview(value: &str, max_lines: usize, max_columns: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for line in value.lines() {
        let rendered = if line.chars().count() > max_columns {
            format!(
                "{}...",
                line.chars()
                    .take(max_columns.saturating_sub(3))
                    .collect::<String>()
            )
        } else {
            line.to_string()
        };
        lines.push(rendered);
        if lines.len() == max_lines {
            lines.push("[truncated]".to_string());
            break;
        }
    }
    if lines.is_empty() {
        lines.push("{}".to_string());
    }
    lines
}
