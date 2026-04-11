use super::*;

pub(crate) fn attachment_preview_status_label(
    preview: &state::ComposerRowAttachmentPreview,
) -> String {
    format!("attachment #{} · {}", preview.index, preview.summary)
}

pub(crate) fn removed_attachment_status_label(
    preview: Option<&state::ComposerRowAttachmentPreview>,
    attachment: &ComposerDraftAttachmentState,
) -> String {
    preview
        .map(attachment_preview_status_label)
        .or_else(|| {
            attachment
                .row_summary()
                .map(|summary| format!("attachment · {summary}"))
        })
        .unwrap_or_else(|| "attachment".to_string())
}

pub(crate) fn external_editor_attachment_status_suffix(
    summary: &state::ComposerAttachmentEditSummary,
) -> String {
    external_editor_attachment_feedback_suffix(summary)
}

pub(crate) fn external_editor_attachment_activity_suffix(
    summary: &state::ComposerAttachmentEditSummary,
) -> String {
    external_editor_attachment_feedback_suffix(summary)
}

pub(crate) fn external_editor_attachment_feedback_suffix(
    summary: &state::ComposerAttachmentEditSummary,
) -> String {
    match (summary.detached.len(), summary.reordered) {
        (0, false) => String::new(),
        (0, true) => " · reordered attachments".to_string(),
        (1, false) => format!(
            " · detached {}",
            attachment_preview_status_label(&summary.detached[0])
        ),
        (count, false) => format!(" · detached {count} attachments"),
        (1, true) => format!(
            " · detached {} and reordered remaining",
            attachment_preview_status_label(&summary.detached[0])
        ),
        (count, true) => format!(" · detached {count} attachments and reordered remaining"),
    }
}

pub(crate) fn preview_path_tail(path: &str) -> String {
    if let Some(segment) = remote_attachment_tail_segment(path) {
        return segment;
    }
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(path)
        .to_string()
}

pub(crate) fn looks_like_local_image_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg")
    )
}

pub(crate) fn is_remote_attachment_url(path: &str) -> bool {
    matches!(path.trim(), value if value.starts_with("http://") || value.starts_with("https://"))
}

pub(crate) fn remote_attachment_tail_segment(path: &str) -> Option<String> {
    let (_, remainder) = path.trim().split_once("://")?;
    let path = remainder
        .split_once('/')
        .map(|(_, path)| path)
        .unwrap_or_default();
    let trimmed = path
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim_matches('/');
    (!trimmed.is_empty()).then(|| {
        trimmed
            .rsplit('/')
            .find(|segment| !segment.is_empty())
            .unwrap_or(trimmed)
            .to_string()
    })
}

pub(crate) fn remote_attachment_file_name(path: &str) -> Option<String> {
    remote_attachment_tail_segment(path).filter(|segment| !segment.is_empty())
}

pub(crate) async fn load_composer_file(
    requested_path: &str,
    ctx: &ToolExecutionContext,
) -> Result<LoadedComposerFile> {
    let resolved_path = resolve_tool_path_against_workspace_root(
        requested_path,
        ctx.effective_root(),
        ctx.container_workdir.as_deref(),
    )?;
    ctx.assert_path_read_allowed(&resolved_path)?;
    let bytes = fs::read(&resolved_path).await?;
    Ok(LoadedComposerFile {
        requested_path: requested_path.to_string(),
        file_name: resolved_path
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_string),
        mime_type: sniff_composer_file_mime(&bytes, &resolved_path).map(str::to_string),
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

pub(crate) fn sniff_composer_file_mime(bytes: &[u8], path: &Path) -> Option<&'static str> {
    if bytes.starts_with(b"%PDF-") {
        return Some("application/pdf");
    }
    match path.extension().and_then(|value| value.to_str()) {
        Some("pdf") => Some("application/pdf"),
        _ => None,
    }
}

pub(crate) fn sniff_remote_image_mime(path: &str) -> Option<&'static str> {
    match remote_attachment_extension(path)?.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

pub(crate) fn sniff_remote_file_mime(path: &str) -> Option<&'static str> {
    match remote_attachment_extension(path)?.as_str() {
        "pdf" => Some("application/pdf"),
        _ => None,
    }
}

pub(crate) fn remote_attachment_extension(path: &str) -> Option<String> {
    let segment = remote_attachment_tail_segment(path)?;
    segment
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .and_then(|extension| {
            let normalized = extension.trim();
            (!normalized.is_empty()).then_some(normalized.to_ascii_lowercase())
        })
}

pub(crate) fn resolve_external_editor_command() -> Result<Vec<String>> {
    let configured = env::var("VISUAL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("EDITOR")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .ok_or_else(|| anyhow!("Cannot open external editor: set $VISUAL or $EDITOR."))?;
    let command = shlex::split(&configured)
        .filter(|segments| !segments.is_empty())
        .ok_or_else(|| anyhow!("Failed to parse external editor command: {configured}"))?;
    Ok(command)
}

pub(crate) fn run_external_editor(seed: &str, editor_command: &[String]) -> Result<String> {
    let file = NamedTempFile::new().context("create external editor temp file")?;
    stdfs::write(file.path(), seed).context("seed external editor temp file")?;

    let (program, args) = editor_command
        .split_first()
        .ok_or_else(|| anyhow!("External editor command is empty"))?;
    let status = ProcessCommand::new(program)
        .args(args)
        .arg(file.path())
        .status()
        .with_context(|| format!("launch external editor `{program}`"))?;
    if !status.success() {
        return Err(anyhow!(
            "External editor exited with status {}",
            status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        ));
    }

    stdfs::read_to_string(file.path()).context("read external editor output")
}
