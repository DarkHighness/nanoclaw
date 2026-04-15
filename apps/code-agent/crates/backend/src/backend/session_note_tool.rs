use crate::backend::session_memory_note::{
    SessionMemoryNotePatch, default_session_memory_note, load_session_memory_note_snapshot,
    patch_session_memory_note, persist_session_memory_note,
};
use agent::memory::MemoryBackend;
use agent::tools::annotations::{builtin_tool_spec, tool_approval_profile};
use agent::tools::registry::Tool;
use agent::types::CallId;
use agent::{ToolCallId, ToolExecutionContext, ToolOutputMode, ToolResult, ToolSpec};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, RwLock};

pub(crate) type SharedMemoryBackendHandle = Arc<RwLock<Option<Arc<dyn MemoryBackend>>>>;

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct MemoryUpdateSessionNoteInput {
    #[serde(default)]
    pub session_title: Option<String>,
    #[serde(default)]
    pub current_state: Option<String>,
    #[serde(default)]
    pub task_specification: Option<String>,
    #[serde(default)]
    pub files_and_functions: Option<String>,
    #[serde(default)]
    pub workflow: Option<String>,
    #[serde(default)]
    pub errors_and_corrections: Option<String>,
    #[serde(default)]
    pub codebase_and_system_documentation: Option<String>,
    #[serde(default)]
    pub learnings: Option<String>,
    #[serde(default)]
    pub key_results: Option<String>,
    #[serde(default)]
    pub worklog: Option<String>,
}

impl MemoryUpdateSessionNoteInput {
    fn into_patch(self) -> SessionMemoryNotePatch {
        SessionMemoryNotePatch {
            session_title: self.session_title,
            current_state: self.current_state,
            task_specification: self.task_specification,
            files_and_functions: self.files_and_functions,
            workflow: self.workflow,
            errors_and_corrections: self.errors_and_corrections,
            codebase_and_system_documentation: self.codebase_and_system_documentation,
            learnings: self.learnings,
            key_results: self.key_results,
            worklog: self.worklog,
        }
    }
}

fn memory_update_session_note_input_schema() -> Value {
    json!({
        "type": "object",
        "description": "Patch the current session note by replacing only the provided sections.",
        "minProperties": 1,
        "properties": {
            "session_title": { "type": "string", "description": "Replace the Session Title section." },
            "current_state": { "type": "string", "description": "Replace the Current State section." },
            "task_specification": { "type": "string", "description": "Replace the Task specification section." },
            "files_and_functions": { "type": "string", "description": "Replace the Files and Functions section." },
            "workflow": { "type": "string", "description": "Replace the Workflow section." },
            "errors_and_corrections": { "type": "string", "description": "Replace the Errors & Corrections section." },
            "codebase_and_system_documentation": { "type": "string", "description": "Replace the Codebase and System Documentation section." },
            "learnings": { "type": "string", "description": "Replace the Learnings section." },
            "key_results": { "type": "string", "description": "Replace the Key results section." },
            "worklog": { "type": "string", "description": "Replace the Worklog section." }
        },
        "additionalProperties": false
    })
}

pub(crate) struct MemoryUpdateSessionNoteTool {
    memory_backend: SharedMemoryBackendHandle,
}

impl MemoryUpdateSessionNoteTool {
    #[must_use]
    pub(crate) fn new(memory_backend: SharedMemoryBackendHandle) -> Self {
        Self { memory_backend }
    }
}

#[async_trait]
impl Tool for MemoryUpdateSessionNoteTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "memory_update_session_note",
            "Update the current session's structured working-memory note after a material continuity change such as a plan pivot, blocker, user correction, or resume-critical next step. Do not use it for routine tool output or repo-obvious edits. Omitted sections stay unchanged.",
            memory_update_session_note_input_schema(),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, true, false),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> agent::tools::Result<ToolResult> {
        let external_call_id = CallId::from(&call_id);
        let input: MemoryUpdateSessionNoteInput = serde_json::from_value(arguments)?;
        let patch = input.into_patch();
        if patch.is_empty() {
            return Err(agent::tools::ToolError::invalid(
                "memory_update_session_note requires at least one section field",
            ));
        }

        let Some(memory_backend) = self.memory_backend.read().unwrap().clone() else {
            return Err(agent::tools::ToolError::invalid_state(
                "memory_update_session_note is unavailable because no memory backend is active",
            ));
        };
        let Some(session_id) = ctx.session_id.clone() else {
            return Err(agent::tools::ToolError::invalid(
                "memory_update_session_note requires an active session_id",
            ));
        };
        let Some(agent_session_id) = ctx.agent_session_id.clone() else {
            return Err(agent::tools::ToolError::invalid(
                "memory_update_session_note requires an active agent_session_id",
            ));
        };

        let existing_snapshot = load_session_memory_note_snapshot(&ctx.workspace_root, &session_id)
            .await
            .map_err(|error| agent::tools::ToolError::invalid_state(error.to_string()))?;
        let current_note = existing_snapshot
            .as_ref()
            .map(|snapshot| snapshot.body.clone())
            .unwrap_or_else(default_session_memory_note);
        let next_note = patch_session_memory_note(&current_note, &patch);
        let updated_sections = patch
            .updated_sections()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let response = persist_session_memory_note(
            &ctx.workspace_root,
            memory_backend.as_ref(),
            &session_id,
            &agent_session_id,
            next_note,
            existing_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.last_summarized_message_id.as_ref()),
            vec!["session-note".to_string(), "model-maintained".to_string()],
        )
        .await
        .map_err(|error| agent::tools::ToolError::invalid_state(error.to_string()))?;

        let text = format!(
            "[memory_update_session_note path={} updated_sections={}]",
            response.path,
            updated_sections.join(", ")
        );
        let structured_output = json!({
            "path": response.path,
            "snapshot_id": response.snapshot_id,
            "metadata": response.metadata,
            "updated_sections": updated_sections,
        });
        let mut result = ToolResult::text(call_id, "memory_update_session_note", text)
            .with_call_id(external_call_id)
            .with_structured_content(structured_output.clone());
        result.metadata = Some(structured_output);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MemoryUpdateSessionNoteTool, SharedMemoryBackendHandle,
        memory_update_session_note_input_schema,
    };
    use crate::backend::session_memory_compaction::session_memory_note_absolute_path;
    use agent::memory::{MemoryBackend, MemoryCoreBackend, MemoryCoreConfig};
    use agent::tools::registry::Tool;
    use agent::types::{AgentSessionId, SessionId};
    use agent::{ToolCallId, ToolExecutionContext};
    use serde_json::json;
    use std::sync::{Arc, RwLock};
    use tempfile::tempdir;
    use tokio::fs;

    fn shared_memory_backend(backend: Arc<dyn MemoryBackend>) -> SharedMemoryBackendHandle {
        Arc::new(RwLock::new(Some(backend)))
    }

    #[tokio::test]
    async fn updates_only_requested_sections_and_preserves_boundary() {
        let dir = tempdir().unwrap();
        let backend: Arc<dyn MemoryBackend> = Arc::new(MemoryCoreBackend::new(
            dir.path().to_path_buf(),
            MemoryCoreConfig::default(),
        ));
        let session_id = SessionId::from("session_1");
        let ctx = ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            session_id: Some(session_id.clone()),
            agent_session_id: Some(AgentSessionId::from("agent_session_1")),
            ..ToolExecutionContext::default()
        };
        fs::create_dir_all(dir.path().join(".nanoclaw/memory/working/sessions"))
            .await
            .unwrap();
        fs::write(
            session_memory_note_absolute_path(dir.path(), &session_id),
            concat!(
                "---\n",
                "scope: working\n",
                "last_summarized_message_id: summary_1\n",
                "---\n\n",
                "# Session Title\n",
                "_A short and distinctive 5-10 word descriptive title for the session. Super info dense, no filler_\n\n",
                "Rollback follow-up\n\n",
                "# Current State\n",
                "_What is actively being worked on right now? Pending tasks not yet completed. Immediate next steps._\n\n",
                "Old state.\n\n",
                "# Files and Functions\n",
                "_What are the important files? In short, what do they contain and why are they relevant?_\n\n",
                "- old/file.rs\n"
            ),
        )
        .await
        .unwrap();

        let tool = MemoryUpdateSessionNoteTool::new(shared_memory_backend(backend));
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "current_state":"New state.",
                    "worklog":"- refreshed continuity note"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(
            result
                .text_content()
                .contains("updated_sections=Current State, Worklog")
        );
        let recorded =
            fs::read_to_string(session_memory_note_absolute_path(dir.path(), &session_id))
                .await
                .unwrap();
        assert!(recorded.contains("last_summarized_message_id: summary_1"));
        assert!(recorded.contains("New state."));
        assert!(recorded.contains("- refreshed continuity note"));
        assert!(recorded.contains("- old/file.rs"));
        assert!(!recorded.contains("Old state."));
    }

    #[tokio::test]
    async fn rejects_empty_patch_requests() {
        let dir = tempdir().unwrap();
        let backend: Arc<dyn MemoryBackend> = Arc::new(MemoryCoreBackend::new(
            dir.path().to_path_buf(),
            MemoryCoreConfig::default(),
        ));
        let tool = MemoryUpdateSessionNoteTool::new(shared_memory_backend(backend));
        let ctx = ToolExecutionContext {
            workspace_root: dir.path().to_path_buf(),
            session_id: Some(SessionId::from("session_1")),
            agent_session_id: Some(AgentSessionId::from("agent_session_1")),
            ..ToolExecutionContext::default()
        };

        let error = tool
            .execute(ToolCallId::new(), json!({}), &ctx)
            .await
            .expect_err("empty patch should fail");

        assert!(error.to_string().contains("at least one section"));
    }

    #[test]
    fn schema_requires_at_least_one_section_field() {
        let schema = memory_update_session_note_input_schema();

        assert_eq!(schema["type"], "object");
        assert_eq!(schema["minProperties"], 1);
        assert_eq!(
            schema["properties"]["current_state"]["description"],
            "Replace the Current State section."
        );
    }
}
