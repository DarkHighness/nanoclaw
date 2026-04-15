use super::*;
use crate::preview::{PreviewCollapse, collapse_preview_text};
use crate::tool_render::{
    ToolDetail, ToolDetailLabel, ToolReview, ToolReviewItemKind,
    compact_successful_exploration_details, tool_argument_details, tool_arguments_preview_lines,
    tool_completion_state, tool_output_details, tool_review, tool_review_from_details,
};
use agent::tools::{
    SessionReviewItem, SessionReviewItemKind, SessionReviewRequest, SessionReviewResult,
    SessionReviewScope, ToolError,
};
use agent::types::{
    CallId, SessionEventEnvelope, SessionEventKind, ToolCall, ToolCallId, ToolName, ToolOrigin,
    ToolResult,
};
use agent::{CodeDiagnosticsTool, CodeIntelBackend, Tool, ToolExecutionContext};
use futures::{StreamExt, stream};
use serde_json::{Value, json};
use std::collections::BTreeSet;

const REVIEW_START_TOOL_NAME: &str = "review_start";
const REVIEW_DIAGNOSTICS_TOOL_NAME: &str = "code_diagnostics";
const REVIEW_DIAGNOSTICS_LIMIT: usize = 32;
const REVIEW_DIAGNOSTIC_PATH_LIMIT: usize = 4;
const REVIEW_DIAGNOSTIC_CONCURRENCY: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReviewBoundary {
    start_index: usize,
    description: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SessionReviewDraft {
    boundary: ReviewBoundary,
    tool_call_count: usize,
    items: Vec<SessionReviewItem>,
    changed_paths: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ReviewDiagnosticsBundle {
    items: Vec<SessionReviewItem>,
    diagnostic_result_count: usize,
}

impl ReviewDiagnosticsBundle {
    fn extend(&mut self, other: Self) {
        self.diagnostic_result_count += other.diagnostic_result_count;
        self.items.extend(other.items);
    }
}

impl CodeAgentSession {
    pub(crate) async fn session_review(
        &self,
        ctx: &ToolExecutionContext,
        request: SessionReviewRequest,
    ) -> agent::tools::Result<SessionReviewResult> {
        let session_id = {
            let runtime = self.runtime.lock().await;
            runtime.session_id()
        };
        let events = self
            .store
            .events(&session_id)
            .await
            .map_err(|error| ToolError::invalid_state(error.to_string()))?;
        let draft = build_session_review_draft(&events, request.scope)?;
        let diagnostics = if request.include_diagnostics {
            let diagnostics_ctx = self.review_diagnostics_context(ctx)?;
            collect_review_diagnostics_from_backend(
                self.code_intel_backend.clone(),
                &diagnostics_ctx,
                &draft.changed_paths,
            )
            .await
        } else {
            ReviewDiagnosticsBundle::default()
        };
        let mut items = draft.items;
        items.extend(diagnostics.items);

        Ok(SessionReviewResult {
            scope: request.scope,
            summary: render_review_summary(
                request.scope,
                draft.boundary.description.as_deref(),
                draft.tool_call_count,
                items.len(),
                request.include_diagnostics,
                diagnostics.diagnostic_result_count,
            ),
            tool_call_count: draft.tool_call_count,
            diagnostics_included: request.include_diagnostics,
            diagnostic_result_count: diagnostics.diagnostic_result_count,
            boundary: draft.boundary.description,
            items,
        })
    }

    fn review_diagnostics_context(
        &self,
        base_ctx: &ToolExecutionContext,
    ) -> agent::tools::Result<ToolExecutionContext> {
        let mut scoped = base_ctx.with_sandbox_policy(
            self.permission_grants
                .effective_sandbox_policy(&base_ctx.sandbox_policy())
                .map_err(|error| ToolError::invalid_state(error.to_string()))?,
        );
        if let (Some(session_id), Some(agent_session_id), Some(turn_id)) = (
            base_ctx.session_id.clone(),
            base_ctx.agent_session_id.clone(),
            base_ctx.turn_id.clone(),
        ) {
            scoped = scoped.with_runtime_scope(
                session_id,
                agent_session_id,
                turn_id,
                ToolName::from(REVIEW_DIAGNOSTICS_TOOL_NAME),
                format!("host-review-diagnostics-{}", new_opaque_id()),
            );
        }
        Ok(scoped)
    }
}

fn build_session_review_draft(
    events: &[SessionEventEnvelope],
    scope: SessionReviewScope,
) -> agent::tools::Result<SessionReviewDraft> {
    let boundary = resolve_review_boundary(events, scope)?;
    let mut items = Vec::new();
    let mut tool_call_count = 0usize;
    let mut changed_paths = BTreeSet::new();

    for event in events.iter().skip(boundary.start_index) {
        match &event.event {
            SessionEventKind::ToolCallCompleted { call, output } => {
                if call.tool_name.as_str() == REVIEW_START_TOOL_NAME {
                    continue;
                }
                items.extend(review_items_from_completed_call(call, output));
                changed_paths.extend(review_changed_paths_from_completed_call(call, output));
                tool_call_count += 1;
            }
            SessionEventKind::ToolCallFailed { call, error } => {
                if call.tool_name.as_str() == REVIEW_START_TOOL_NAME {
                    continue;
                }
                items.extend(review_items_from_failed_call(call, error));
                tool_call_count += 1;
            }
            _ => {}
        }
    }

    Ok(SessionReviewDraft {
        boundary,
        tool_call_count,
        items,
        changed_paths,
    })
}

fn resolve_review_boundary(
    events: &[SessionEventEnvelope],
    scope: SessionReviewScope,
) -> agent::tools::Result<ReviewBoundary> {
    match scope {
        SessionReviewScope::LatestTurn => {
            let start_index = events
                .iter()
                .rposition(|event| matches!(event.event, SessionEventKind::UserPromptSubmit { .. }))
                .map_or(0, |index| index + 1);
            Ok(ReviewBoundary {
                start_index,
                description: Some("latest user prompt".to_string()),
            })
        }
        SessionReviewScope::SinceCheckpoint => events
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, event)| match &event.event {
                SessionEventKind::CheckpointCreated { checkpoint } => Some(ReviewBoundary {
                    start_index: index + 1,
                    description: Some(format!(
                        "{} ({})",
                        checkpoint.checkpoint_id, checkpoint.summary
                    )),
                }),
                _ => None,
            })
            .ok_or_else(|| {
                ToolError::invalid_state(
                    "review_start with scope=since_checkpoint requires at least one checkpoint in the current session",
                )
            }),
    }
}

fn review_items_from_completed_call(
    call: &ToolCall,
    output: &ToolResult,
) -> Vec<SessionReviewItem> {
    let argument_preview = tool_arguments_preview_lines(call.tool_name.as_str(), &call.arguments);
    let mut detail_lines = tool_argument_details(&argument_preview);
    let completion =
        tool_completion_state(call.tool_name.as_str(), output.structured_content.as_ref());
    detail_lines.extend(tool_output_details(
        call.tool_name.as_str(),
        &output.text_content(),
        output.structured_content.as_ref(),
    ));
    compact_successful_exploration_details(&mut detail_lines, completion);
    let review = tool_review(call.tool_name.as_str(), output.structured_content.as_ref())
        .or_else(|| tool_review_from_details(&detail_lines));
    session_review_items(call.tool_name.as_str(), review)
}

fn review_items_from_failed_call(call: &ToolCall, error: &str) -> Vec<SessionReviewItem> {
    let argument_preview = tool_arguments_preview_lines(call.tool_name.as_str(), &call.arguments);
    let mut items = session_review_items(
        call.tool_name.as_str(),
        tool_review_from_details(&tool_argument_details(&argument_preview)),
    );
    let detail_lines = vec![ToolDetail::LabeledValue {
        label: ToolDetailLabel::Result,
        value: error.trim().to_string(),
    }];
    if let Some(review) = tool_review_from_details(&detail_lines) {
        items.extend(review.items.into_iter().map(|item| SessionReviewItem {
            title: format!("{} · {}", call.tool_name, item.title),
            kind: SessionReviewItemKind::Stderr,
            preview_lines: collapse_review_lines(&item.preview_lines),
        }));
    } else {
        items.push(SessionReviewItem {
            title: format!("{} · Result", call.tool_name),
            kind: SessionReviewItemKind::Stderr,
            preview_lines: collapse_review_lines(&[error.trim().to_string()]),
        });
    }
    items
}

fn review_changed_paths_from_completed_call(call: &ToolCall, output: &ToolResult) -> Vec<String> {
    let mut paths = output
        .structured_content
        .as_ref()
        .and_then(|value| value.get("file_diffs"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|diff| {
            diff.get("path")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    if !paths.is_empty() {
        return paths;
    }

    if !matches!(
        call.tool_name.as_str(),
        "write" | "edit" | "patch_files" | "notebook_edit"
    ) {
        return Vec::new();
    }

    if let Some(path) = output
        .structured_content
        .as_ref()
        .and_then(|value| value.get("requested_path"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
    {
        paths.push(path.to_string());
    }
    paths
}

async fn collect_review_diagnostics_from_backend(
    backend: Arc<dyn CodeIntelBackend>,
    ctx: &ToolExecutionContext,
    changed_paths: &BTreeSet<String>,
) -> ReviewDiagnosticsBundle {
    let requests = review_diagnostic_requests(changed_paths);
    let mut diagnostics = stream::iter(requests)
        .map(|path| {
            let backend = backend.clone();
            let ctx = ctx.clone();
            async move { collect_single_review_diagnostic(backend, &ctx, path).await }
        })
        .buffered(REVIEW_DIAGNOSTIC_CONCURRENCY)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .fold(
            ReviewDiagnosticsBundle::default(),
            |mut aggregate, bundle| {
                aggregate.extend(bundle);
                aggregate
            },
        );

    let omitted = changed_paths
        .len()
        .saturating_sub(REVIEW_DIAGNOSTIC_PATH_LIMIT);
    if omitted > 0 {
        diagnostics.items.push(SessionReviewItem {
            title: "code_diagnostics · Note".to_string(),
            kind: SessionReviewItemKind::Neutral,
            preview_lines: vec![format!(
                "Skipped {omitted} additional changed file(s) to keep the diagnostics review bounded."
            )],
        });
    }
    diagnostics
}

fn review_diagnostic_requests(changed_paths: &BTreeSet<String>) -> Vec<Option<String>> {
    if changed_paths.is_empty() {
        return vec![None];
    }

    // Review bundles need to stay lightweight enough to render inline, so V2
    // samples only the first few changed files instead of recursively dumping
    // every workspace diagnostic into the overlay.
    changed_paths
        .iter()
        .take(REVIEW_DIAGNOSTIC_PATH_LIMIT)
        .cloned()
        .map(Some)
        .collect()
}

async fn collect_single_review_diagnostic(
    backend: Arc<dyn CodeIntelBackend>,
    ctx: &ToolExecutionContext,
    path: Option<String>,
) -> ReviewDiagnosticsBundle {
    let arguments = review_diagnostic_arguments(path.as_deref());
    let call = ToolCall {
        id: ToolCallId::new(),
        call_id: CallId::new(),
        tool_name: ToolName::from(REVIEW_DIAGNOSTICS_TOOL_NAME),
        arguments: arguments.clone(),
        origin: ToolOrigin::Local,
    };
    let tool = CodeDiagnosticsTool::with_backend(backend);
    match tool.execute(ToolCallId::new(), arguments, ctx).await {
        Ok(output) => {
            let diagnostic_result_count = output
                .structured_content
                .as_ref()
                .and_then(|value| value.get("result_count"))
                .and_then(Value::as_u64)
                .map_or(0, |count| count as usize);
            let items = if diagnostic_result_count == 0 {
                Vec::new()
            } else {
                review_items_from_diagnostics_result(&call, &output)
            };
            ReviewDiagnosticsBundle {
                items,
                diagnostic_result_count,
            }
        }
        Err(error) => ReviewDiagnosticsBundle {
            items: vec![SessionReviewItem {
                title: "code_diagnostics · Result".to_string(),
                kind: SessionReviewItemKind::Stderr,
                preview_lines: collapse_review_lines(&[render_diagnostics_error(
                    path.as_deref(),
                    &error.to_string(),
                )]),
            }],
            diagnostic_result_count: 0,
        },
    }
}

fn review_diagnostic_arguments(path: Option<&str>) -> Value {
    match path {
        Some(path) => json!({
            "path": path,
            "limit": REVIEW_DIAGNOSTICS_LIMIT,
        }),
        None => json!({
            "limit": REVIEW_DIAGNOSTICS_LIMIT,
        }),
    }
}

fn review_items_from_diagnostics_result(
    call: &ToolCall,
    output: &ToolResult,
) -> Vec<SessionReviewItem> {
    let detail_lines = tool_output_details(
        REVIEW_DIAGNOSTICS_TOOL_NAME,
        &output.text_content(),
        output.structured_content.as_ref(),
    );
    session_review_items(
        call.tool_name.as_str(),
        tool_review_from_details(&detail_lines),
    )
}

fn render_diagnostics_error(path: Option<&str>, error: &str) -> String {
    match path.map(str::trim).filter(|value| !value.is_empty()) {
        Some(path) => format!("{path}: {error}"),
        None => error.to_string(),
    }
}

fn session_review_items(title_prefix: &str, review: Option<ToolReview>) -> Vec<SessionReviewItem> {
    review
        .map(|review| {
            review
                .items
                .into_iter()
                .map(|item| SessionReviewItem {
                    title: format!("{title_prefix} · {}", item.title),
                    kind: map_review_item_kind(item.preview_kind),
                    preview_lines: item.preview_lines,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn map_review_item_kind(kind: ToolReviewItemKind) -> SessionReviewItemKind {
    match kind {
        ToolReviewItemKind::Neutral => SessionReviewItemKind::Neutral,
        ToolReviewItemKind::Command => SessionReviewItemKind::Command,
        ToolReviewItemKind::Stdout => SessionReviewItemKind::Stdout,
        ToolReviewItemKind::Stderr => SessionReviewItemKind::Stderr,
        ToolReviewItemKind::Diff => SessionReviewItemKind::Diff,
    }
}

fn collapse_review_lines(lines: &[String]) -> Vec<String> {
    let body = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>();
    if body.is_empty() {
        return Vec::new();
    }
    collapse_preview_text(&body.join("\n"), 48, 120, PreviewCollapse::HeadTail)
}

fn render_review_summary(
    scope: SessionReviewScope,
    boundary: Option<&str>,
    tool_call_count: usize,
    item_count: usize,
    diagnostics_included: bool,
    diagnostic_result_count: usize,
) -> String {
    let scope_text = match scope {
        SessionReviewScope::LatestTurn => "latest turn",
        SessionReviewScope::SinceCheckpoint => "checkpoint boundary",
    };
    let tool_summary = match (
        tool_call_count,
        boundary.filter(|value| !value.trim().is_empty()),
    ) {
        (0, Some(boundary)) => format!("No completed tool activity found since {boundary}."),
        (0, None) => format!("No completed tool activity found in the {scope_text}."),
        (_, Some(boundary)) => format!(
            "Reviewed {tool_call_count} completed tool call(s) and surfaced {item_count} section(s) since {boundary}."
        ),
        _ => format!(
            "Reviewed {tool_call_count} completed tool call(s) and surfaced {item_count} section(s) from the {scope_text}."
        ),
    };
    if !diagnostics_included {
        return tool_summary;
    }
    if diagnostic_result_count == 0 {
        format!("{tool_summary} Current diagnostics reported 0 issue(s).")
    } else {
        format!("{tool_summary} Current diagnostics reported {diagnostic_result_count} issue(s).")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_session_review_draft, collect_review_diagnostics_from_backend,
        resolve_review_boundary, review_diagnostic_requests,
    };
    use agent::ToolExecutionContext;
    use agent::tools::{
        CodeLocation, CodeNavigationTarget, CodeReference, SandboxPolicy, SessionReviewItemKind,
        SessionReviewRequest, SessionReviewScope,
    };
    use agent::types::{
        AgentSessionId, CallId, CheckpointId, CheckpointOrigin, CheckpointRecord, CheckpointScope,
        EventId, SessionEventEnvelope, SessionEventKind, SessionId, SubmittedPromptSnapshot,
        ToolCall, ToolCallId, ToolName, ToolOrigin, ToolResult,
    };
    use agent::{CodeDiagnostic, CodeDiagnosticSeverity, CodeDiagnosticSource, CodeIntelBackend};
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use std::collections::BTreeSet;
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn envelope(event: SessionEventKind, index: u64) -> SessionEventEnvelope {
        SessionEventEnvelope {
            id: EventId::new(),
            timestamp_ms: index as u128,
            session_id: SessionId::from("session_1"),
            agent_session_id: AgentSessionId::from("agent_1"),
            turn_id: None,
            tool_call_id: None,
            event,
        }
    }

    fn prompt(index: u64, text: &str) -> SessionEventEnvelope {
        envelope(
            SessionEventKind::UserPromptSubmit {
                prompt: SubmittedPromptSnapshot {
                    text: text.to_string(),
                    attachments: Vec::new(),
                },
            },
            index,
        )
    }

    fn checkpoint(index: u64, summary: &str) -> SessionEventEnvelope {
        envelope(
            SessionEventKind::CheckpointCreated {
                checkpoint: CheckpointRecord {
                    checkpoint_id: CheckpointId::from("checkpoint_1"),
                    session_id: SessionId::from("session_1"),
                    agent_session_id: AgentSessionId::from("agent_1"),
                    scope: CheckpointScope::Both,
                    origin: CheckpointOrigin::FileTool {
                        tool_name: ToolName::from("write"),
                    },
                    summary: summary.to_string(),
                    created_at_unix_s: index,
                    rollback_message_id: None,
                    prompt_message_id: None,
                    changed_files: Vec::new(),
                },
            },
            index,
        )
    }

    fn completed(
        index: u64,
        tool_name: &str,
        arguments: Value,
        output: ToolResult,
    ) -> SessionEventEnvelope {
        envelope(
            SessionEventKind::ToolCallCompleted {
                call: ToolCall {
                    id: ToolCallId::from(format!("tool_{index}")),
                    call_id: CallId::from(format!("call_{index}")),
                    tool_name: ToolName::from(tool_name),
                    arguments,
                    origin: ToolOrigin::Local,
                },
                output,
            },
            index,
        )
    }

    fn failed(index: u64, tool_name: &str, error: &str) -> SessionEventEnvelope {
        envelope(
            SessionEventKind::ToolCallFailed {
                call: ToolCall {
                    id: ToolCallId::from(format!("tool_fail_{index}")),
                    call_id: CallId::from(format!("call_fail_{index}")),
                    tool_name: ToolName::from(tool_name),
                    arguments: json!({"path": "src/lib.rs"}),
                    origin: ToolOrigin::Local,
                },
                error: error.to_string(),
            },
            index,
        )
    }

    #[derive(Default)]
    struct DiagnosticsStubBackend;

    #[async_trait]
    impl CodeIntelBackend for DiagnosticsStubBackend {
        fn name(&self) -> &'static str {
            "diagnostics_stub"
        }

        async fn workspace_symbols(
            &self,
            _query: &str,
            _limit: usize,
            _ctx: &ToolExecutionContext,
        ) -> agent::tools::Result<Vec<agent::CodeSymbol>> {
            Ok(Vec::new())
        }

        async fn document_symbols(
            &self,
            _path: &Path,
            _limit: usize,
            _ctx: &ToolExecutionContext,
        ) -> agent::tools::Result<Vec<agent::CodeSymbol>> {
            Ok(Vec::new())
        }

        async fn definitions(
            &self,
            _target: &CodeNavigationTarget,
            _limit: usize,
            _ctx: &ToolExecutionContext,
        ) -> agent::tools::Result<Vec<agent::CodeSymbol>> {
            Ok(Vec::new())
        }

        async fn references(
            &self,
            _target: &CodeNavigationTarget,
            _include_declaration: bool,
            _limit: usize,
            _ctx: &ToolExecutionContext,
        ) -> agent::tools::Result<Vec<CodeReference>> {
            Ok(Vec::new())
        }

        async fn diagnostics(
            &self,
            path: Option<&Path>,
            _limit: usize,
            _ctx: &ToolExecutionContext,
        ) -> agent::tools::Result<Vec<CodeDiagnostic>> {
            let Some(path) = path else {
                return Ok(vec![CodeDiagnostic {
                    location: CodeLocation {
                        path: "src/main.rs".to_string(),
                        line: 3,
                        column: 1,
                    },
                    severity: CodeDiagnosticSeverity::Warning,
                    message: "workspace warning".to_string(),
                    source: CodeDiagnosticSource::Lsp,
                    provider: Some("rust-analyzer".to_string()),
                }]);
            };
            let normalized = path.to_string_lossy().replace('\\', "/");
            if normalized.ends_with("/src/lib.rs") {
                Ok(vec![CodeDiagnostic {
                    location: CodeLocation {
                        path: "src/lib.rs".to_string(),
                        line: 7,
                        column: 5,
                    },
                    severity: CodeDiagnosticSeverity::Error,
                    message: "missing semicolon".to_string(),
                    source: CodeDiagnosticSource::Lsp,
                    provider: Some("rust-analyzer".to_string()),
                }])
            } else {
                Ok(Vec::new())
            }
        }
    }

    fn review_tool_context(root: &Path) -> ToolExecutionContext {
        ToolExecutionContext {
            workspace_root: root.to_path_buf(),
            worktree_root: Some(root.to_path_buf()),
            effective_sandbox_policy: Some(SandboxPolicy::permissive()),
            workspace_only: true,
            ..ToolExecutionContext::default()
        }
    }

    #[test]
    fn latest_turn_boundary_uses_last_prompt() {
        let events = vec![
            prompt(1, "first"),
            completed(
                2,
                "exec_command",
                json!({"cmd": "cargo test"}),
                ToolResult::text(ToolCallId::from("result_1"), "exec_command", "ok"),
            ),
            prompt(3, "second"),
        ];
        let boundary = resolve_review_boundary(&events, SessionReviewScope::LatestTurn)
            .expect("boundary should resolve");
        assert_eq!(boundary.start_index, 3);
    }

    #[test]
    fn since_checkpoint_requires_boundary() {
        let events = vec![prompt(1, "first")];
        let error = resolve_review_boundary(&events, SessionReviewScope::SinceCheckpoint)
            .expect_err("boundary should be required");
        assert!(
            error
                .to_string()
                .contains("requires at least one checkpoint")
        );
    }

    #[test]
    fn session_review_draft_collects_terminal_tool_activity_and_changed_paths() {
        let file_output = ToolResult::text(ToolCallId::from("result_write"), "write", "updated")
            .with_structured_content(json!({
                "summary": "Updated src/lib.rs",
                "file_diffs": [
                    {
                        "path": "src/lib.rs",
                        "preview": "@@ -1 +1 @@\n-old\n+new"
                    }
                ]
            }));
        let events = vec![
            prompt(1, "first"),
            completed(
                2,
                "exec_command",
                json!({"cmd": "cargo test"}),
                ToolResult::text(ToolCallId::from("result_old"), "exec_command", "old"),
            ),
            checkpoint(3, "before write"),
            completed(4, "write", json!({"path": "src/lib.rs"}), file_output),
            failed(5, "exec_command", "boom"),
        ];

        let draft = build_session_review_draft(&events, SessionReviewScope::SinceCheckpoint)
            .expect("review should succeed");

        assert_eq!(draft.tool_call_count, 2);
        assert_eq!(
            draft.boundary.description.as_deref(),
            Some("checkpoint_1 (before write)")
        );
        assert!(draft.changed_paths.contains("src/lib.rs"));
        assert!(
            draft
                .items
                .iter()
                .any(|item| item.kind == SessionReviewItemKind::Diff)
        );
        assert!(
            draft
                .items
                .iter()
                .any(|item| item.title == "exec_command · Result")
        );
    }

    #[test]
    fn diagnostics_requests_fall_back_to_workspace_without_changed_files() {
        let requests = review_diagnostic_requests(&BTreeSet::new());
        assert_eq!(requests, vec![None]);
    }

    #[tokio::test]
    async fn diagnostics_bundle_adds_current_diagnostics_for_changed_paths() {
        let tempdir = tempdir().expect("tempdir");
        let backend: Arc<dyn CodeIntelBackend> = Arc::new(DiagnosticsStubBackend);
        let changed_paths = BTreeSet::from(["src/lib.rs".to_string()]);
        let bundle = collect_review_diagnostics_from_backend(
            backend,
            &review_tool_context(tempdir.path()),
            &changed_paths,
        )
        .await;

        assert_eq!(bundle.diagnostic_result_count, 1);
        assert!(
            bundle
                .items
                .iter()
                .any(|item| item.title == "code_diagnostics · Output")
        );
    }

    #[test]
    fn review_request_defaults_enable_diagnostics() {
        let request = SessionReviewRequest::default();
        assert_eq!(request.scope, SessionReviewScope::LatestTurn);
        assert!(request.include_diagnostics);
    }
}
