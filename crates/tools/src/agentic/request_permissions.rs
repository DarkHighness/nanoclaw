use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::registry::Tool;
use crate::{
    PermissionRequest, RequestPermissionProfile, RequestPermissionsArgs, Result, ToolError,
    ToolExecutionContext, granted_permissions_are_subset, normalize_request_permission_profile,
    request_permission_profile_from_granted,
};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::Serialize;
use serde_json::Value;
use types::{MessagePart, ToolCallId, ToolOutputMode, ToolResult, ToolSpec};

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct RequestPermissionsToolOutput {
    requested: RequestPermissionProfile,
    granted: RequestPermissionProfile,
    scope: crate::PermissionGrantScope,
}

#[derive(Clone, Debug, Default)]
pub struct RequestPermissionsTool;

impl RequestPermissionsTool {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for RequestPermissionsTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "request_permissions",
            "Request additional filesystem or network permissions from the user. Granted permissions apply automatically to later tool calls in the current turn, or for the rest of the session if the host grants session scope.",
            serde_json::to_value(schema_for!(RequestPermissionsArgs))
                .expect("request_permissions schema"),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, true, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(RequestPermissionsToolOutput))
                .expect("request_permissions output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let args: RequestPermissionsArgs = serde_json::from_value(arguments)?;
        if args.permissions.is_empty() {
            return Err(ToolError::invalid(
                "request_permissions requires at least one permission",
            ));
        }

        let handler = ctx.permission_request_handler.as_ref().ok_or_else(|| {
            ToolError::invalid_state(
                "request_permissions is unavailable without a host permission handler",
            )
        })?;
        let requested =
            normalize_request_permission_profile(&args.permissions, ctx.effective_root())?;
        let response = handler
            .request_permissions(PermissionRequest {
                reason: args.reason.clone(),
                permissions: requested.clone(),
            })
            .await?;
        if !granted_permissions_are_subset(&requested, &response.permissions) {
            return Err(ToolError::invalid_state(
                "request_permissions host granted permissions outside the requested subset",
            ));
        }

        let structured_output = RequestPermissionsToolOutput {
            requested: request_permission_profile_from_granted(&requested),
            granted: request_permission_profile_from_granted(&response.permissions),
            scope: response.scope,
        };

        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "request_permissions".into(),
            parts: vec![MessagePart::text(format_permission_response(
                &structured_output,
            ))],
            attachments: Vec::new(),
            structured_content: Some(
                serde_json::to_value(&structured_output)
                    .expect("request_permissions structured output"),
            ),
            continuation: None,
            metadata: Some(serde_json::json!({
                "requested": structured_output.requested,
                "granted": structured_output.granted,
                "scope": structured_output.scope,
            })),
            is_error: false,
        })
    }
}

fn format_permission_response(output: &RequestPermissionsToolOutput) -> String {
    if output.granted.is_empty() {
        return "The user did not grant any additional permissions. Continue without those permissions or choose a narrower request.".to_string();
    }

    let scope = match output.scope {
        crate::PermissionGrantScope::Turn => "current turn",
        crate::PermissionGrantScope::Session => "current session",
    };
    format!(
        "The user granted additional permissions for the {scope}. Granted profile: {}",
        serde_json::to_string(&output.granted).unwrap_or_else(|_| "<unavailable>".to_string())
    )
}

#[cfg(test)]
mod tests {
    use super::RequestPermissionsTool;
    use crate::{
        GrantedPermissionResponse, PermissionGrantScope, PermissionRequest,
        PermissionRequestHandler, RequestPermissionsArgs, Result, Tool, ToolExecutionContext,
    };
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::tempdir;
    use types::ToolCallId;

    struct StaticPermissionHandler;

    #[async_trait]
    impl PermissionRequestHandler for StaticPermissionHandler {
        async fn request_permissions(
            &self,
            request: PermissionRequest,
        ) -> Result<GrantedPermissionResponse> {
            Ok(GrantedPermissionResponse {
                permissions: request.permissions,
                scope: PermissionGrantScope::Session,
            })
        }
    }

    #[tokio::test]
    async fn request_permissions_returns_granted_profile() {
        let tool = RequestPermissionsTool::new();
        let workspace = tempdir().unwrap();
        let result = tool
            .execute(
                ToolCallId::new(),
                serde_json::to_value(RequestPermissionsArgs {
                    reason: Some("need write access".to_string()),
                    permissions: crate::RequestPermissionProfile {
                        file_system: Some(crate::FileSystemPermissionRequest {
                            read: None,
                            write: Some(vec!["tmp".to_string()]),
                        }),
                        network: None,
                    },
                })
                .unwrap(),
                &ToolExecutionContext {
                    workspace_root: workspace.path().to_path_buf(),
                    worktree_root: Some(workspace.path().to_path_buf()),
                    permission_request_handler: Some(Arc::new(StaticPermissionHandler)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(
            result
                .structured_content
                .as_ref()
                .expect("structured output")["scope"],
            json!("session")
        );
        assert!(result.text_content().contains("current session"));
    }
}
