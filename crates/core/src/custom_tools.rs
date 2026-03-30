//! Host-side loader for lightweight workspace custom tools.
//!
//! This intentionally sits next to plugin boot instead of inside the runtime or
//! tool crates. Loose tool manifests are a host discovery concern, but once
//! loaded they should execute through the exact same `ToolRegistry`,
//! `ToolExecutionContext`, approval metadata, and sandboxed process path as
//! builtin tools.

use crate::AgentWorkspaceLayout;
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::time::{Duration, timeout};
use tools::{
    DynamicTool, DynamicToolHandler, ExecRequest, ExecutionOrigin, ProcessExecutor, ProcessStdio,
    RuntimeScope, ToolError, ToolExecutionContext, ToolRegistry,
};
use types::{
    CallId, MessagePart, ToolApprovalProfile, ToolAttachment, ToolCallId, ToolContinuation,
    ToolOutputMode, ToolResult, ToolSource, ToolSpec,
};

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_TIMEOUT_MS: u64 = 5 * 60_000;
const MAX_CAPTURE_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CustomToolLoadOutcome {
    pub loaded_tools: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn register_workspace_custom_tools(
    workspace_root: &Path,
    process_executor: Option<Arc<dyn ProcessExecutor>>,
    tools: &ToolRegistry,
) -> Result<CustomToolLoadOutcome> {
    let tools_dir = AgentWorkspaceLayout::new(workspace_root).tools_dir();
    let manifest_paths = discover_manifest_paths(&tools_dir)
        .with_context(|| format!("failed to scan custom tools under {}", tools_dir.display()))?;
    if manifest_paths.is_empty() {
        return Ok(CustomToolLoadOutcome::default());
    }

    let Some(process_executor) = process_executor else {
        return Ok(CustomToolLoadOutcome {
            loaded_tools: Vec::new(),
            warnings: vec![format!(
                "custom tools found under {} but host process surfaces are disabled",
                tools_dir.display()
            )],
        });
    };

    let mut outcome = CustomToolLoadOutcome::default();
    for manifest_path in manifest_paths {
        match load_custom_tool(manifest_path.clone(), process_executor.clone()) {
            Ok((name, tool)) => match tools.try_register_arc(Arc::new(tool)) {
                Ok(()) => outcome.loaded_tools.push(name),
                Err(error) => outcome.warnings.push(format!(
                    "failed to register custom tool from {}: {error:#}",
                    manifest_path.display()
                )),
            },
            Err(error) => outcome.warnings.push(format!(
                "failed to load custom tool from {}: {error:#}",
                manifest_path.display()
            )),
        }
    }
    outcome.loaded_tools.sort();
    Ok(outcome)
}

fn discover_manifest_paths(tools_dir: &Path) -> Result<Vec<PathBuf>> {
    if !tools_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut manifests = Vec::new();
    for entry in std::fs::read_dir(tools_dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_file()
            && path.extension().and_then(|ext| ext.to_str()) == Some("toml")
        {
            manifests.push(path);
            continue;
        }
        if entry.file_type()?.is_dir() {
            let manifest = path.join("tool.toml");
            if manifest.is_file() {
                manifests.push(manifest);
            }
        }
    }
    manifests.sort();
    Ok(manifests)
}

fn load_custom_tool(
    manifest_path: PathBuf,
    process_executor: Arc<dyn ProcessExecutor>,
) -> Result<(String, DynamicTool)> {
    let manifest_dir = manifest_path
        .parent()
        .context("custom tool manifest has no parent directory")?
        .to_path_buf();
    let raw = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    let manifest: CustomToolManifest = toml::from_str(&raw)
        .with_context(|| format!("failed to parse {}", manifest_path.display()))?;
    let definition = CustomToolDefinition::from_manifest(manifest, &manifest_path, &manifest_dir)?;
    let name = definition.spec.name.to_string();
    let handler = definition.into_handler(process_executor);
    Ok((
        name,
        DynamicTool::from_tool_spec(handler.spec, handler.handler),
    ))
}

struct CustomToolDefinition {
    spec: ToolSpec,
    runtime: CustomToolRuntime,
}

impl CustomToolDefinition {
    fn from_manifest(
        manifest: CustomToolManifest,
        manifest_path: &Path,
        manifest_dir: &Path,
    ) -> Result<Self> {
        let default_name =
            if manifest_path.file_name().and_then(|value| value.to_str()) == Some("tool.toml") {
                manifest_dir
                    .file_name()
                    .and_then(|value| value.to_str())
                    .context("custom tool package directory must have a valid UTF-8 name")?
                    .to_string()
            } else {
                manifest_path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .context("custom tool manifest file must have a valid UTF-8 stem")?
                    .to_string()
            };
        let tool_name = manifest.name.unwrap_or(default_name);
        validate_identifier(&tool_name, "custom tool name")?;
        if manifest.description.trim().is_empty() {
            anyhow::bail!("custom tool description cannot be empty");
        }
        if manifest.program.trim().is_empty() {
            anyhow::bail!("custom tool program cannot be empty");
        }

        let spec = ToolSpec::function(
            tool_name.clone(),
            manifest.description.trim().to_string(),
            build_input_schema(&manifest.parameters)?,
            manifest.output_mode,
            types::ToolOrigin::Local,
            ToolSource::Dynamic,
        )
        .with_aliases(validate_aliases(&manifest.aliases)?)
        .with_parallel_support(manifest.supports_parallel_tool_calls)
        .with_approval(build_approval_profile(manifest.approval)?);

        Ok(Self {
            spec,
            runtime: CustomToolRuntime {
                tool_name,
                manifest_path: manifest_path.to_path_buf(),
                tool_dir: manifest_dir.to_path_buf(),
                program: resolve_program_path(manifest_dir, &manifest.program)?,
                args: resolve_program_args(manifest_dir, &manifest.args),
                env: manifest.env,
                timeout_ms: manifest
                    .timeout_ms
                    .unwrap_or(DEFAULT_TIMEOUT_MS)
                    .clamp(1, MAX_TIMEOUT_MS),
            },
        })
    }

    fn into_handler(self, process_executor: Arc<dyn ProcessExecutor>) -> LoadedCustomTool {
        let runtime = Arc::new(self.runtime);
        let spec = self.spec;
        let handler: DynamicToolHandler = Arc::new(move |call_id, arguments, ctx| {
            let runtime = runtime.clone();
            let process_executor = process_executor.clone();
            Box::pin(async move {
                execute_custom_tool(
                    runtime.as_ref(),
                    process_executor.as_ref(),
                    call_id,
                    arguments,
                    ctx,
                )
                .await
            })
        });
        LoadedCustomTool { spec, handler }
    }
}

struct LoadedCustomTool {
    spec: ToolSpec,
    handler: DynamicToolHandler,
}

#[derive(Clone)]
struct CustomToolRuntime {
    tool_name: String,
    manifest_path: PathBuf,
    tool_dir: PathBuf,
    program: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    timeout_ms: u64,
}

#[derive(Debug, Deserialize)]
struct CustomToolManifest {
    #[serde(default)]
    name: Option<String>,
    description: String,
    program: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    parameters: Vec<CustomToolParameter>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    output_mode: ToolOutputMode,
    #[serde(default)]
    supports_parallel_tool_calls: bool,
    #[serde(default)]
    approval: CustomToolApprovalConfig,
}

#[derive(Debug, Deserialize)]
struct CustomToolParameter {
    name: String,
    description: String,
    #[serde(rename = "type")]
    ty: CustomToolParameterType,
    #[serde(default)]
    required: bool,
    #[serde(default, rename = "enum")]
    enum_values: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CustomToolParameterType {
    String,
    Integer,
    Number,
    Boolean,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CustomToolApprovalConfig {
    read_only: Option<bool>,
    mutates_state: Option<bool>,
    idempotent: Option<bool>,
    open_world: Option<bool>,
    needs_network: bool,
    needs_host_escape: Option<bool>,
    approval_message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CustomToolCommandOutput {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    parts: Vec<MessagePart>,
    #[serde(default)]
    structured_content: Option<Value>,
    #[serde(default)]
    metadata: Option<Value>,
    #[serde(default)]
    continuation: Option<ToolContinuation>,
    #[serde(default)]
    attachments: Vec<ToolAttachment>,
    #[serde(default)]
    is_error: bool,
}

async fn execute_custom_tool(
    runtime: &CustomToolRuntime,
    process_executor: &dyn ProcessExecutor,
    call_id: ToolCallId,
    arguments: Value,
    ctx: ToolExecutionContext,
) -> tools::Result<ToolResult> {
    let external_call_id = CallId::from(&call_id);
    let cwd = ctx.effective_root().to_path_buf();
    let mut env = runtime.env.clone();
    env.insert("NANOCLAW_TOOL_NAME".to_string(), runtime.tool_name.clone());
    env.insert(
        "NANOCLAW_TOOL_MANIFEST".to_string(),
        runtime.manifest_path.display().to_string(),
    );
    env.insert(
        "NANOCLAW_TOOL_DIR".to_string(),
        runtime.tool_dir.display().to_string(),
    );
    env.insert(
        "NANOCLAW_WORKSPACE_ROOT".to_string(),
        ctx.workspace_root.display().to_string(),
    );

    let payload = json!({
        "arguments": arguments,
        "tool_name": runtime.tool_name,
        "tool_call_id": call_id,
        "workspace_root": ctx.workspace_root.display().to_string(),
        "session_id": ctx.session_id.as_ref().map(ToString::to_string),
        "agent_session_id": ctx.agent_session_id.as_ref().map(ToString::to_string),
        "turn_id": ctx.turn_id.as_ref().map(ToString::to_string),
        "agent_id": ctx.agent_id.as_ref().map(ToString::to_string),
        "tool_dir": runtime.tool_dir.display().to_string(),
        "manifest_path": runtime.manifest_path.display().to_string(),
    });
    let stdin_payload = serde_json::to_vec(&payload).map_err(|error| {
        ToolError::invalid_state(format!("failed to encode custom tool payload: {error}"))
    })?;

    let mut child = process_executor
        .prepare(ExecRequest {
            program: runtime.program.clone(),
            args: runtime.args.clone(),
            cwd: Some(cwd.clone()),
            env,
            stdin: ProcessStdio::Piped,
            stdout: ProcessStdio::Piped,
            stderr: ProcessStdio::Piped,
            kill_on_drop: true,
            origin: ExecutionOrigin::HostUtility {
                name: format!("custom_tool:{}", runtime.tool_name),
            },
            runtime_scope: runtime_scope_from_context(&ctx),
            sandbox_policy: ctx.sandbox_policy(),
        })
        .map_err(|error| ToolError::invalid_state(error.to_string()))?
        .spawn()
        .map_err(|error| ToolError::invalid_state(error.to_string()))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&stdin_payload).await.map_err(|error| {
            ToolError::invalid_state(format!("failed to write custom tool stdin: {error}"))
        })?;
    }

    let output = match timeout(
        Duration::from_millis(runtime.timeout_ms),
        child.wait_with_output(),
    )
    .await
    {
        Ok(result) => result.map_err(|error| ToolError::invalid_state(error.to_string()))?,
        Err(_) => {
            return Ok(ToolResult {
                id: call_id,
                call_id: external_call_id,
                tool_name: runtime.tool_name.clone().into(),
                parts: vec![MessagePart::text(format!(
                    "[custom tool {}]\nCommand timed out after {}ms.\nmanifest> {}",
                    runtime.tool_name,
                    runtime.timeout_ms,
                    runtime.manifest_path.display()
                ))],
                attachments: Vec::new(),
                structured_content: None,
                continuation: None,
                metadata: Some(json!({
                    "manifest_path": runtime.manifest_path.display().to_string(),
                    "program": runtime.program,
                    "timeout_ms": runtime.timeout_ms,
                    "timed_out": true,
                })),
                is_error: true,
            });
        }
    };

    let stdout = truncate_bytes(&output.stdout);
    let stderr = truncate_bytes(&output.stderr);
    let parsed = decode_command_output(&stdout);
    let mut metadata = Map::new();
    metadata.insert(
        "manifest_path".to_string(),
        Value::String(runtime.manifest_path.display().to_string()),
    );
    metadata.insert(
        "program".to_string(),
        Value::String(runtime.program.clone()),
    );
    metadata.insert(
        "exit_code".to_string(),
        Value::Number(output.status.code().unwrap_or(-1).into()),
    );
    if !stderr.trim().is_empty() {
        metadata.insert("stderr".to_string(), Value::String(stderr.clone()));
    }

    if let Some(parsed) = parsed {
        let result_metadata = merge_metadata(parsed.metadata, metadata);
        let parts = if !parsed.parts.is_empty() {
            parsed.parts
        } else {
            vec![MessagePart::text(parsed.text.unwrap_or_default())]
        };
        return Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: runtime.tool_name.clone().into(),
            parts,
            attachments: parsed.attachments,
            structured_content: parsed.structured_content,
            continuation: parsed.continuation,
            metadata: (!result_metadata.is_empty()).then(|| Value::Object(result_metadata)),
            is_error: parsed.is_error || !output.status.success(),
        });
    }

    let text = render_command_output(&runtime.tool_name, &stdout, &stderr);
    Ok(ToolResult {
        id: call_id,
        call_id: external_call_id,
        tool_name: runtime.tool_name.clone().into(),
        parts: vec![MessagePart::text(text)],
        attachments: Vec::new(),
        structured_content: None,
        continuation: None,
        metadata: Some(Value::Object(metadata)),
        is_error: !output.status.success(),
    })
}

fn decode_command_output(stdout: &str) -> Option<CustomToolCommandOutput> {
    let value: Value = serde_json::from_str(stdout.trim()).ok()?;
    let object = value.as_object()?;
    let has_known_key = object.keys().any(|key| {
        matches!(
            key.as_str(),
            "text"
                | "parts"
                | "structured_content"
                | "metadata"
                | "continuation"
                | "attachments"
                | "is_error"
        )
    });
    has_known_key
        .then(|| serde_json::from_value(value).ok())
        .flatten()
}

fn truncate_bytes(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes).to_string();
    if text.len() <= MAX_CAPTURE_BYTES {
        text
    } else {
        text[..MAX_CAPTURE_BYTES].to_string()
    }
}

fn render_command_output(tool_name: &str, stdout: &str, stderr: &str) -> String {
    let stdout = stdout.trim();
    let stderr = stderr.trim();
    match (stdout.is_empty(), stderr.is_empty()) {
        (false, true) => stdout.to_string(),
        (true, false) => format!("[custom tool {tool_name} stderr]\n{stderr}"),
        (false, false) => format!("{stdout}\n\n[stderr]\n{stderr}"),
        (true, true) => String::new(),
    }
}

fn runtime_scope_from_context(ctx: &ToolExecutionContext) -> RuntimeScope {
    RuntimeScope {
        session_id: ctx.session_id.clone(),
        agent_session_id: ctx.agent_session_id.clone(),
        turn_id: ctx.turn_id.clone(),
        tool_name: ctx.tool_name.clone().map(|name| name.to_string()),
        tool_call_id: ctx.tool_call_id.clone(),
    }
}

fn merge_metadata(existing: Option<Value>, extra: Map<String, Value>) -> Map<String, Value> {
    let mut merged = match existing {
        Some(Value::Object(object)) => object,
        Some(other) => {
            let mut object = Map::new();
            object.insert("tool_output".to_string(), other);
            object
        }
        None => Map::new(),
    };
    for (key, value) in extra {
        merged.entry(key).or_insert(value);
    }
    merged
}

fn resolve_program_path(tool_dir: &Path, program: &str) -> Result<String> {
    let path = Path::new(program);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        tool_dir.join(path)
    };
    Ok(resolved.display().to_string())
}

fn resolve_program_args(tool_dir: &Path, args: &[String]) -> Vec<String> {
    args.iter()
        .map(|arg| {
            if arg.starts_with("./") || arg.starts_with("../") {
                tool_dir.join(arg).display().to_string()
            } else {
                arg.clone()
            }
        })
        .collect()
}

fn build_input_schema(parameters: &[CustomToolParameter]) -> Result<Value> {
    let mut properties = Map::new();
    let mut required = Vec::new();
    let mut seen = BTreeSet::new();
    for parameter in parameters {
        validate_identifier(&parameter.name, "custom tool parameter")?;
        if !seen.insert(parameter.name.clone()) {
            anyhow::bail!("duplicate custom tool parameter `{}`", parameter.name);
        }
        if parameter.description.trim().is_empty() {
            anyhow::bail!(
                "custom tool parameter `{}` must have a description",
                parameter.name
            );
        }
        let mut schema = Map::new();
        schema.insert(
            "type".to_string(),
            Value::String(parameter.ty.json_type().to_string()),
        );
        schema.insert(
            "description".to_string(),
            Value::String(parameter.description.trim().to_string()),
        );
        if !parameter.enum_values.is_empty() {
            if !matches!(parameter.ty, CustomToolParameterType::String) {
                anyhow::bail!(
                    "custom tool parameter `{}` only supports enum values for string type",
                    parameter.name
                );
            }
            schema.insert(
                "enum".to_string(),
                Value::Array(
                    parameter
                        .enum_values
                        .iter()
                        .map(|value| Value::String(value.clone()))
                        .collect(),
                ),
            );
        }
        if parameter.required {
            required.push(Value::String(parameter.name.clone()));
        }
        properties.insert(parameter.name.clone(), Value::Object(schema));
    }
    Ok(json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    }))
}

fn build_approval_profile(config: CustomToolApprovalConfig) -> Result<ToolApprovalProfile> {
    let read_only = config.read_only.unwrap_or(false);
    let mutates_state = config.mutates_state.unwrap_or(!read_only);
    if read_only && mutates_state {
        anyhow::bail!("custom tool approval cannot be both read_only and mutates_state");
    }
    let mut profile = ToolApprovalProfile::new(
        read_only,
        mutates_state,
        config.idempotent,
        config.open_world.unwrap_or(true),
    )
    .with_network(config.needs_network)
    .with_host_escape(config.needs_host_escape.unwrap_or(true));
    if let Some(message) = config
        .approval_message
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        profile = profile.with_approval_message(message);
    }
    Ok(profile)
}

fn validate_aliases(aliases: &[String]) -> Result<Vec<types::ToolName>> {
    let mut seen = BTreeSet::new();
    let mut validated = Vec::new();
    for alias in aliases {
        validate_identifier(alias, "custom tool alias")?;
        if seen.insert(alias.clone()) {
            validated.push(alias.clone().into());
        }
    }
    Ok(validated)
}

fn validate_identifier(value: &str, label: &str) -> Result<()> {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        anyhow::bail!("{label} cannot be empty");
    };
    if !first.is_ascii_lowercase() {
        anyhow::bail!("{label} `{value}` must start with an ASCII lowercase letter");
    }
    if !chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_') {
        anyhow::bail!("{label} `{value}` must be snake_case");
    }
    Ok(())
}

impl CustomToolParameterType {
    fn json_type(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Integer => "integer",
            Self::Number => "number",
            Self::Boolean => "boolean",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::register_workspace_custom_tools;
    use crate::AgentWorkspaceLayout;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tools::{HostProcessExecutor, ToolExecutionContext, ToolRegistry};
    use types::ToolCallId;

    #[tokio::test]
    async fn workspace_custom_tools_register_and_execute() {
        let workspace = tempdir().unwrap();
        let layout = AgentWorkspaceLayout::new(workspace.path());
        layout.ensure_standard_layout().unwrap();
        let tool_dir = layout.tools_dir().join("echo_payload");
        std::fs::create_dir_all(&tool_dir).unwrap();
        let script_path = tool_dir.join("run.sh");
        std::fs::write(
            &script_path,
            "#!/bin/sh\ncat >/dev/null\nprintf '{\"text\":\"custom ok\",\"metadata\":{\"kind\":\"custom\"}}'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&script_path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&script_path, permissions).unwrap();
        }
        std::fs::write(
            tool_dir.join("tool.toml"),
            r#"
description = "Echo a stable response."
program = "./run.sh"

[[parameters]]
name = "message"
type = "string"
description = "Message to send."
required = true

[approval]
read_only = true
mutates_state = false
idempotent = true
"#,
        )
        .unwrap();

        let registry = ToolRegistry::new();
        let outcome = register_workspace_custom_tools(
            workspace.path(),
            Some(Arc::new(HostProcessExecutor)),
            &registry,
        )
        .unwrap();
        assert_eq!(
            outcome.loaded_tools,
            vec!["echo_payload".to_string()],
            "{:?}",
            outcome.warnings
        );
        assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);

        let tool = registry
            .get("echo_payload")
            .expect("custom tool should register");
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({"message": "hello"}),
                &ToolExecutionContext {
                    workspace_root: workspace.path().to_path_buf(),
                    workspace_only: false,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(result.text_content(), "custom ok");
        assert_eq!(result.metadata.unwrap()["kind"], "custom");
    }

    #[test]
    fn invalid_custom_tool_warns_without_stopping_load() {
        let workspace = tempdir().unwrap();
        let layout = AgentWorkspaceLayout::new(workspace.path());
        layout.ensure_standard_layout().unwrap();
        std::fs::write(
            layout.tools_dir().join("broken.toml"),
            r#"
description = ""
program = ""
"#,
        )
        .unwrap();

        let registry = ToolRegistry::new();
        let outcome = register_workspace_custom_tools(
            workspace.path(),
            Some(Arc::new(HostProcessExecutor)),
            &registry,
        )
        .unwrap();

        assert!(outcome.loaded_tools.is_empty());
        assert_eq!(outcome.warnings.len(), 1);
        assert!(registry.names().is_empty());
    }
}
