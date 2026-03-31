use crate::ToolExecutionContext;
use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::file_activity::FileActivityObserver;
use crate::fs::resolve_tool_path_against_workspace_root;
use crate::registry::Tool;
use crate::{Result, ToolError};
use async_trait::async_trait;
use base64::Engine;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::fs;
use types::{MessagePart, ToolAttachment, ToolCallId, ToolOutputMode, ToolResult, ToolSpec};

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct ViewImageToolInput {
    pub path: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct ViewImageToolOutput {
    requested_path: String,
    resolved_path: String,
    mime_type: String,
    byte_length: usize,
}

#[derive(Clone, Default)]
pub struct ViewImageTool {
    activity_observer: Option<Arc<dyn FileActivityObserver>>,
}

impl ViewImageTool {
    #[must_use]
    pub fn new() -> Self {
        Self {
            activity_observer: None,
        }
    }

    #[must_use]
    pub fn with_file_activity_observer(activity_observer: Arc<dyn FileActivityObserver>) -> Self {
        Self {
            activity_observer: Some(activity_observer),
        }
    }
}

#[async_trait]
impl Tool for ViewImageTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "view_image",
            "Read a local image file and return it as image content for visual inspection.",
            serde_json::to_value(schema_for!(ViewImageToolInput)).expect("view_image schema"),
            ToolOutputMode::ContentParts,
            tool_approval_profile(true, false, true, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(ViewImageToolOutput))
                .expect("view_image output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: ViewImageToolInput = serde_json::from_value(arguments)?;
        let resolved = resolve_tool_path_against_workspace_root(
            &input.path,
            ctx.effective_root(),
            ctx.container_workdir.as_deref(),
        )?;
        ctx.assert_path_read_allowed(&resolved)?;
        let bytes = fs::read(&resolved).await?;
        let mime = sniff_image_mime(&bytes, &resolved)
            .ok_or_else(|| ToolError::invalid("view_image: file is not a supported image"))?;
        if let Some(observer) = &self.activity_observer {
            observer.did_open(resolved.clone());
        }

        let byte_length = bytes.len();
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "view_image".into(),
            parts: vec![
                MessagePart::text(format!("Viewed image file [{mime}]")),
                MessagePart::Image {
                    mime_type: mime.to_string(),
                    data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                },
            ],
            attachments: vec![ToolAttachment {
                kind: "image".to_string(),
                name: resolved
                    .file_name()
                    .and_then(|value| value.to_str())
                    .map(str::to_string),
                mime_type: Some(mime.to_string()),
                uri: Some(resolved.display().to_string()),
                metadata: Some(serde_json::json!({
                    "requested_path": input.path,
                    "resolved_path": resolved,
                    "byte_length": byte_length,
                })),
            }],
            structured_content: Some(
                serde_json::to_value(ViewImageToolOutput {
                    requested_path: input.path.clone(),
                    resolved_path: resolved.display().to_string(),
                    mime_type: mime.to_string(),
                    byte_length,
                })
                .expect("view_image output"),
            ),
            continuation: None,
            metadata: Some(serde_json::json!({ "path": resolved })),
            is_error: false,
        })
    }
}

fn sniff_image_mime(bytes: &[u8], path: &std::path::Path) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.starts_with(b"RIFF") && bytes.len() >= 12 && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }

    match path.extension().and_then(|value| value.to_str()) {
        Some("png") => Some("image/png"),
        Some("jpg") | Some("jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{ViewImageTool, ViewImageToolInput};
    use crate::{Tool, ToolExecutionContext};
    use nanoclaw_test_support::run_current_thread_test;
    use types::{MessagePart, ToolCallId};

    macro_rules! bounded_async_test {
        (async fn $name:ident() $body:block) => {
            #[test]
            fn $name() {
                run_current_thread_test(async $body);
            }
        };
    }

    fn context(root: &std::path::Path) -> ToolExecutionContext {
        ToolExecutionContext {
            workspace_root: root.to_path_buf(),
            workspace_only: true,
            ..Default::default()
        }
    }

    bounded_async_test!(
        async fn view_image_returns_image_part_for_png_files() {
            let dir = tempfile::tempdir().unwrap();
            tokio::fs::write(dir.path().join("sample.png"), b"\x89PNG\r\n\x1a\npayload")
                .await
                .unwrap();

            let tool = ViewImageTool::new();
            let result = tool
                .execute(
                    ToolCallId::new(),
                    serde_json::to_value(ViewImageToolInput {
                        path: "sample.png".to_string(),
                    })
                    .unwrap(),
                    &context(dir.path()),
                )
                .await
                .unwrap();

            assert_eq!(result.tool_name.as_str(), "view_image");
            assert_eq!(result.parts.len(), 2);
            assert!(matches!(result.parts[1], MessagePart::Image { .. }));
            assert_eq!(result.structured_content.unwrap()["mime_type"], "image/png");
        }
    );

    bounded_async_test!(
        async fn view_image_rejects_non_image_files() {
            let dir = tempfile::tempdir().unwrap();
            tokio::fs::write(dir.path().join("sample.txt"), "not an image")
                .await
                .unwrap();

            let tool = ViewImageTool::new();
            let error = tool
                .execute(
                    ToolCallId::new(),
                    serde_json::json!({"path": "sample.txt"}),
                    &context(dir.path()),
                )
                .await
                .expect_err("non-image input should fail");

            assert!(error.to_string().contains("supported image"));
        }
    );
}
