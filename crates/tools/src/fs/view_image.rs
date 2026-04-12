use crate::ToolExecutionContext;
use crate::fs::resolve_tool_path_against_workspace_root;
use crate::{Result, ToolError};
use base64::Engine;
use std::path::PathBuf;
use tokio::fs;
use types::MessagePart;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedToolImage {
    pub(crate) requested_path: String,
    pub(crate) resolved_path: PathBuf,
    pub(crate) mime_type: String,
    pub(crate) byte_length: usize,
    pub(crate) data_base64: String,
}

impl LoadedToolImage {
    pub fn message_part(&self) -> MessagePart {
        MessagePart::Image {
            mime_type: self.mime_type.clone(),
            data_base64: self.data_base64.clone(),
        }
    }
}

pub async fn load_tool_image(
    requested_path: &str,
    ctx: &ToolExecutionContext,
) -> Result<LoadedToolImage> {
    let resolved_path = resolve_tool_path_against_workspace_root(
        requested_path,
        ctx.effective_root(),
        ctx.container_workdir.as_deref(),
    )?;
    ctx.assert_path_read_allowed(&resolved_path)?;
    let bytes = fs::read(&resolved_path).await?;
    let mime_type = sniff_image_mime(&bytes, &resolved_path)
        .ok_or_else(|| ToolError::invalid("image file is not a supported image"))?;
    Ok(LoadedToolImage {
        requested_path: requested_path.to_string(),
        resolved_path,
        mime_type: mime_type.to_string(),
        byte_length: bytes.len(),
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

pub fn sniff_image_mime(bytes: &[u8], path: &std::path::Path) -> Option<&'static str> {
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
    use super::load_tool_image;
    use crate::ToolExecutionContext;
    use nanoclaw_test_support::run_current_thread_test;
    use types::MessagePart;

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
        async fn load_tool_image_returns_image_part_for_png_files() {
            let dir = tempfile::tempdir().unwrap();
            tokio::fs::write(dir.path().join("sample.png"), b"\x89PNG\r\n\x1a\npayload")
                .await
                .unwrap();

            let image = load_tool_image("sample.png", &context(dir.path()))
                .await
                .unwrap();

            assert_eq!(image.mime_type, "image/png");
            assert_eq!(image.requested_path, "sample.png");
            assert_eq!(
                image.message_part(),
                MessagePart::Image {
                    mime_type: "image/png".to_string(),
                    data_base64: image.data_base64.clone(),
                }
            );
        }
    );

    bounded_async_test!(
        async fn load_tool_image_rejects_non_image_files() {
            let dir = tempfile::tempdir().unwrap();
            tokio::fs::write(dir.path().join("sample.txt"), "not an image")
                .await
                .unwrap();

            let error = load_tool_image("sample.txt", &context(dir.path()))
                .await
                .expect_err("non-image input should fail");

            assert!(error.to_string().contains("supported image"));
        }
    );
}
