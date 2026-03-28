use crate::{Result, RuntimeError};
use async_trait::async_trait;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tools::{NetworkPolicy, ToolExecutionContext};
use types::{
    HookContext, HookEffect, HookExecutionPolicy, HookHandler, HookHostApiGrant, HookNetworkPolicy,
    HookRegistration, HookResult,
};
use wasmtime::{Caller, Config, Engine, Linker, Module, Store};

const DEFAULT_WASM_TIMEOUT_MS: u64 = 500;
const HOST_MODULE: &str = "nanoclaw:host";

#[async_trait]
pub trait WasmHookExecutor: Send + Sync {
    async fn execute(
        &self,
        registration: &HookRegistration,
        context: HookContext,
    ) -> Result<HookResult>;
}

#[derive(Default)]
pub struct DefaultWasmHookExecutor;

#[async_trait]
impl WasmHookExecutor for DefaultWasmHookExecutor {
    async fn execute(
        &self,
        registration: &HookRegistration,
        context: HookContext,
    ) -> Result<HookResult> {
        let registration = registration.clone();
        tokio::task::spawn_blocking(move || execute_wasm_hook(&registration, context))
            .await
            .map_err(|error| RuntimeError::hook(format!("wasm hook task failed: {error}")))?
    }
}

struct WasmHostState {
    context_json: Vec<u8>,
    emitted_effects: Vec<HookEffect>,
    tool_context: ToolExecutionContext,
    execution: HookExecutionPolicy,
}

fn execute_wasm_hook(registration: &HookRegistration, context: HookContext) -> Result<HookResult> {
    let HookHandler::Wasm(wasm) = &registration.handler else {
        return Err(RuntimeError::hook(format!(
            "hook `{}` is not a wasm hook",
            registration.name
        )));
    };
    let execution = registration
        .execution
        .clone()
        .ok_or_else(|| RuntimeError::hook("wasm hooks require execution policy"))?;
    let tool_context = tool_context_for_execution(&execution);
    let module_path = PathBuf::from(&wasm.module);
    tool_context
        .assert_path_execute_allowed(&module_path)
        .map_err(|error| RuntimeError::hook(error.to_string()))?;

    let mut config = Config::new();
    config.epoch_interruption(true);
    config.consume_fuel(true);
    let engine = Engine::new(&config).map_err(|error| {
        RuntimeError::hook(format!("failed to initialize wasm engine: {error}"))
    })?;
    let module = Module::from_file(&engine, &module_path)
        .map_err(|error| RuntimeError::hook(format!("failed to load wasm module: {error}")))?;
    let mut linker = Linker::new(&engine);
    bind_host_functions(&mut linker)?;

    let timeout_ms = registration.timeout_ms.unwrap_or(DEFAULT_WASM_TIMEOUT_MS);
    let engine_for_timer = engine.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(timeout_ms));
        engine_for_timer.increment_epoch();
    });

    let mut store = Store::new(
        &engine,
        WasmHostState {
            context_json: serde_json::to_vec(&context).map_err(|error| {
                RuntimeError::hook(format!("failed to serialize hook context: {error}"))
            })?,
            emitted_effects: Vec::new(),
            tool_context,
            execution,
        },
    );
    store
        .set_fuel(50_000)
        .map_err(|error| RuntimeError::hook(format!("failed to configure wasm fuel: {error}")))?;
    store.set_epoch_deadline(1);
    let instance = linker
        .instantiate(&mut store, &module)
        .map_err(map_wasm_error)?;
    let function = instance
        .get_typed_func::<(), ()>(&mut store, &wasm.entrypoint)
        .map_err(|error| {
            RuntimeError::hook(format!(
                "missing wasm entrypoint `{}`: {error}",
                wasm.entrypoint
            ))
        })?;
    function.call(&mut store, ()).map_err(map_wasm_error)?;

    Ok(HookResult {
        effects: store.data().emitted_effects.clone(),
    })
}

fn bind_host_functions(linker: &mut Linker<WasmHostState>) -> Result<()> {
    linker
        .func_wrap(
            HOST_MODULE,
            "get_context_len",
            |caller: Caller<'_, WasmHostState>| {
                require_host_api(&caller, HookHostApiGrant::GetHookContext)?;
                Ok::<i32, wasmtime::Error>(caller.data().context_json.len() as i32)
            },
        )
        .map_err(map_wasm_error)?;
    linker
        .func_wrap(
            HOST_MODULE,
            "read_context",
            |mut caller: Caller<'_, WasmHostState>, ptr: i32| {
                require_host_api(&caller, HookHostApiGrant::GetHookContext)?;
                let bytes = caller.data().context_json.clone();
                write_guest_bytes(&mut caller, ptr, &bytes)?;
                Ok::<i32, wasmtime::Error>(bytes.len() as i32)
            },
        )
        .map_err(map_wasm_error)?;
    linker
        .func_wrap(
            HOST_MODULE,
            "emit_effect",
            |mut caller: Caller<'_, WasmHostState>, ptr: i32, len: i32| {
                require_host_api(&caller, HookHostApiGrant::EmitHookEffect)?;
                let bytes = read_guest_bytes(&mut caller, ptr, len)?;
                let effect = serde_json::from_slice::<HookEffect>(&bytes).map_err(|error| {
                    wasmtime::Error::msg(format!("invalid hook effect payload: {error}"))
                })?;
                caller.data_mut().emitted_effects.push(effect);
                Ok::<i32, wasmtime::Error>(0)
            },
        )
        .map_err(map_wasm_error)?;
    linker
        .func_wrap(
            HOST_MODULE,
            "log",
            |mut caller: Caller<'_, WasmHostState>,
             level_ptr: i32,
             level_len: i32,
             message_ptr: i32,
             message_len: i32| {
                require_host_api(&caller, HookHostApiGrant::Log)?;
                let _level = read_guest_string(&mut caller, level_ptr, level_len)?;
                let _message = read_guest_string(&mut caller, message_ptr, message_len)?;
                Ok::<i32, wasmtime::Error>(0)
            },
        )
        .map_err(map_wasm_error)?;
    linker
        .func_wrap(
            HOST_MODULE,
            "read_file_len",
            |mut caller: Caller<'_, WasmHostState>, path_ptr: i32, path_len: i32| {
                let bytes = read_file_bytes(&mut caller, path_ptr, path_len)?;
                Ok::<i32, wasmtime::Error>(bytes.len() as i32)
            },
        )
        .map_err(map_wasm_error)?;
    linker
        .func_wrap(
            HOST_MODULE,
            "read_file",
            |mut caller: Caller<'_, WasmHostState>, path_ptr: i32, path_len: i32, out_ptr: i32| {
                let bytes = read_file_bytes(&mut caller, path_ptr, path_len)?;
                write_guest_bytes(&mut caller, out_ptr, &bytes)?;
                Ok::<i32, wasmtime::Error>(bytes.len() as i32)
            },
        )
        .map_err(map_wasm_error)?;
    linker
        .func_wrap(
            HOST_MODULE,
            "write_file",
            |mut caller: Caller<'_, WasmHostState>,
             path_ptr: i32,
             path_len: i32,
             content_ptr: i32,
             content_len: i32| {
                require_host_api(&caller, HookHostApiGrant::WriteFile)?;
                let path = resolve_guest_path(&mut caller, path_ptr, path_len)?;
                caller
                    .data()
                    .tool_context
                    .assert_path_write_allowed(&path)
                    .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
                let bytes = read_guest_bytes(&mut caller, content_ptr, content_len)?;
                fs::write(&path, bytes).map_err(|error| {
                    wasmtime::Error::msg(format!("failed to write file: {error}"))
                })?;
                Ok::<i32, wasmtime::Error>(0)
            },
        )
        .map_err(map_wasm_error)?;
    linker
        .func_wrap(
            HOST_MODULE,
            "list_dir_len",
            |mut caller: Caller<'_, WasmHostState>, path_ptr: i32, path_len: i32| {
                let listing = list_dir_bytes(&mut caller, path_ptr, path_len)?;
                Ok::<i32, wasmtime::Error>(listing.len() as i32)
            },
        )
        .map_err(map_wasm_error)?;
    linker
        .func_wrap(
            HOST_MODULE,
            "list_dir",
            |mut caller: Caller<'_, WasmHostState>, path_ptr: i32, path_len: i32, out_ptr: i32| {
                let listing = list_dir_bytes(&mut caller, path_ptr, path_len)?;
                write_guest_bytes(&mut caller, out_ptr, &listing)?;
                Ok::<i32, wasmtime::Error>(listing.len() as i32)
            },
        )
        .map_err(map_wasm_error)?;
    Ok(())
}

fn require_host_api(
    caller: &Caller<'_, WasmHostState>,
    grant: HookHostApiGrant,
) -> std::result::Result<(), wasmtime::Error> {
    if caller.data().execution.allows_host_api(grant) {
        Ok(())
    } else {
        Err(wasmtime::Error::msg(format!(
            "host API `{grant:?}` is not granted"
        )))
    }
}

fn read_file_bytes(
    caller: &mut Caller<'_, WasmHostState>,
    path_ptr: i32,
    path_len: i32,
) -> std::result::Result<Vec<u8>, wasmtime::Error> {
    require_host_api(caller, HookHostApiGrant::ReadFile)?;
    let path = resolve_guest_path(caller, path_ptr, path_len)?;
    caller
        .data()
        .tool_context
        .assert_path_read_allowed(&path)
        .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
    fs::read(&path).map_err(|error| wasmtime::Error::msg(format!("failed to read file: {error}")))
}

fn list_dir_bytes(
    caller: &mut Caller<'_, WasmHostState>,
    path_ptr: i32,
    path_len: i32,
) -> std::result::Result<Vec<u8>, wasmtime::Error> {
    require_host_api(caller, HookHostApiGrant::ListDir)?;
    let path = resolve_guest_path(caller, path_ptr, path_len)?;
    caller
        .data()
        .tool_context
        .assert_path_read_allowed(&path)
        .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
    let mut entries = fs::read_dir(&path)
        .map_err(|error| wasmtime::Error::msg(format!("failed to list dir: {error}")))?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    entries.sort();
    Ok(entries.join("\n").into_bytes())
}

fn resolve_guest_path(
    caller: &mut Caller<'_, WasmHostState>,
    path_ptr: i32,
    path_len: i32,
) -> std::result::Result<PathBuf, wasmtime::Error> {
    let path = read_guest_string(caller, path_ptr, path_len)?;
    Ok(PathBuf::from(path))
}

fn read_guest_string(
    caller: &mut Caller<'_, WasmHostState>,
    ptr: i32,
    len: i32,
) -> std::result::Result<String, wasmtime::Error> {
    let bytes = read_guest_bytes(caller, ptr, len)?;
    String::from_utf8(bytes)
        .map_err(|error| wasmtime::Error::msg(format!("guest provided invalid UTF-8: {error}")))
}

fn read_guest_bytes(
    caller: &mut Caller<'_, WasmHostState>,
    ptr: i32,
    len: i32,
) -> std::result::Result<Vec<u8>, wasmtime::Error> {
    let memory = guest_memory(caller)?;
    let mut bytes = vec![0u8; len as usize];
    memory
        .read(caller, ptr as usize, &mut bytes)
        .map_err(|error| wasmtime::Error::msg(format!("failed to read guest memory: {error}")))?;
    Ok(bytes)
}

fn write_guest_bytes(
    caller: &mut Caller<'_, WasmHostState>,
    ptr: i32,
    bytes: &[u8],
) -> std::result::Result<(), wasmtime::Error> {
    let memory = guest_memory(caller)?;
    memory
        .write(caller, ptr as usize, bytes)
        .map_err(|error| wasmtime::Error::msg(format!("failed to write guest memory: {error}")))
}

fn guest_memory(
    caller: &mut Caller<'_, WasmHostState>,
) -> std::result::Result<wasmtime::Memory, wasmtime::Error> {
    caller
        .get_export("memory")
        .and_then(|export| export.into_memory())
        .ok_or_else(|| wasmtime::Error::msg("wasm module does not export memory"))
}

fn tool_context_for_execution(execution: &HookExecutionPolicy) -> ToolExecutionContext {
    let workspace_root = execution
        .plugin_root
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    ToolExecutionContext {
        workspace_root: workspace_root.clone(),
        worktree_root: Some(workspace_root),
        read_only_roots: execution.read_roots.clone(),
        writable_roots: execution.write_roots.clone(),
        exec_roots: execution.exec_roots.clone(),
        network_policy: Some(match &execution.network {
            HookNetworkPolicy::Deny => NetworkPolicy::Off,
            HookNetworkPolicy::Allow => NetworkPolicy::Full,
            HookNetworkPolicy::AllowDomains { domains } => {
                NetworkPolicy::AllowDomains(domains.clone())
            }
        }),
        workspace_only: true,
        ..Default::default()
    }
}

fn map_wasm_error(error: impl std::fmt::Display) -> RuntimeError {
    let message = error.to_string();
    if message.contains("interrupt") || message.contains("fuel") {
        RuntimeError::hook("wasm hook timed out")
    } else {
        RuntimeError::hook(format!("wasm hook failed: {message}"))
    }
}

#[cfg(test)]
mod tests {
    use super::{DefaultWasmHookExecutor, WasmHookExecutor};
    use types::{
        HookContext, HookEvent, HookExecutionPolicy, HookHandler, HookHostApiGrant,
        HookRegistration, HookResult, MessagePart, MessageRole, RunId, SessionId, WasmHookHandler,
    };

    fn write_wasm_module(
        dir: &tempfile::TempDir,
        name: &str,
        wat_source: &str,
    ) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, wat::parse_str(wat_source).unwrap()).unwrap();
        path
    }

    fn wat_string(value: &str) -> String {
        value.replace('\\', "\\\\").replace('"', "\\\"")
    }

    fn base_registration(module: &std::path::Path, entrypoint: &str) -> HookRegistration {
        HookRegistration {
            name: "wasm".to_string(),
            event: HookEvent::UserPromptSubmit,
            matcher: None,
            handler: HookHandler::Wasm(WasmHookHandler {
                module: module.to_string_lossy().to_string(),
                entrypoint: entrypoint.to_string(),
            }),
            timeout_ms: Some(50),
            execution: Some(HookExecutionPolicy {
                plugin_id: Some("team-policy".to_string()),
                plugin_root: Some(module.parent().unwrap().to_path_buf()),
                exec_roots: vec![module.parent().unwrap().to_path_buf()],
                ..HookExecutionPolicy::default()
            }),
        }
    }

    #[tokio::test]
    async fn wasm_hook_executes_and_emits_effects() {
        let dir = tempfile::tempdir().unwrap();
        let effect_json = "{\"kind\":\"append_message\",\"role\":\"system\",\"parts\":[{\"type\":\"text\",\"text\":\"from wasm\"}]}";
        let module = write_wasm_module(
            &dir,
            "ok.wasm",
            &format!(
                r#"(module
                    (import "{host}" "get_context_len" (func $get_context_len (result i32)))
                    (import "{host}" "read_context" (func $read_context (param i32) (result i32)))
                    (import "{host}" "emit_effect" (func $emit_effect (param i32 i32) (result i32)))
                    (memory (export "memory") 1)
                    (data (i32.const 0) "{effect_data}")
                    (func (export "on_user_prompt")
                        i32.const 256
                        call $read_context
                        drop
                        i32.const 0
                        i32.const {effect_len}
                        call $emit_effect
                        drop))
                "#,
                host = super::HOST_MODULE,
                effect_data = wat_string(effect_json),
                effect_len = effect_json.len(),
            ),
        );
        let mut registration = base_registration(&module, "on_user_prompt");
        registration.execution = Some(HookExecutionPolicy {
            plugin_id: Some("team-policy".to_string()),
            plugin_root: Some(dir.path().to_path_buf()),
            exec_roots: vec![dir.path().to_path_buf()],
            host_api_grants: vec![
                HookHostApiGrant::GetHookContext,
                HookHostApiGrant::EmitHookEffect,
            ],
            ..HookExecutionPolicy::default()
        });

        let output = DefaultWasmHookExecutor
            .execute(
                &registration,
                HookContext {
                    event: HookEvent::UserPromptSubmit,
                    run_id: RunId::from("run_1"),
                    session_id: SessionId::from("session_1"),
                    turn_id: None,
                    fields: Default::default(),
                    payload: serde_json::json!({"prompt":"hello"}),
                },
            )
            .await
            .unwrap();

        assert_eq!(
            output,
            HookResult {
                effects: vec![types::HookEffect::AppendMessage {
                    role: MessageRole::System,
                    parts: vec![MessagePart::text("from wasm")],
                }],
            }
        );
    }

    #[tokio::test]
    async fn wasm_hook_denies_ungranted_file_read() {
        let dir = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        std::fs::write(external.path().join("secret.txt"), "shh").unwrap();
        let secret_path = external.path().join("secret.txt");
        let module = write_wasm_module(
            &dir,
            "deny_read.wasm",
            &format!(
                r#"(module
                    (import "{host}" "read_file_len" (func $read_file_len (param i32 i32) (result i32)))
                    (memory (export "memory") 1)
                    (data (i32.const 0) "{path}")
                    (func (export "on_user_prompt")
                        i32.const 0
                        i32.const {path_len}
                        call $read_file_len
                        drop))
                "#,
                host = super::HOST_MODULE,
                path = wat_string(&secret_path.to_string_lossy()),
                path_len = secret_path.to_string_lossy().len(),
            ),
        );
        let mut registration = base_registration(&module, "on_user_prompt");
        registration.execution = Some(HookExecutionPolicy {
            plugin_id: Some("team-policy".to_string()),
            plugin_root: Some(dir.path().to_path_buf()),
            exec_roots: vec![dir.path().to_path_buf()],
            host_api_grants: vec![HookHostApiGrant::ReadFile],
            ..HookExecutionPolicy::default()
        });

        let error = DefaultWasmHookExecutor
            .execute(
                &registration,
                HookContext {
                    event: HookEvent::UserPromptSubmit,
                    run_id: RunId::from("run_1"),
                    session_id: SessionId::from("session_1"),
                    turn_id: None,
                    fields: Default::default(),
                    payload: serde_json::json!({}),
                },
            )
            .await
            .unwrap_err();
        assert!(!error.to_string().is_empty());
    }

    #[tokio::test]
    async fn wasm_hook_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let module = write_wasm_module(
            &dir,
            "loop.wasm",
            r#"(module
                (memory (export "memory") 1)
                (func (export "on_user_prompt")
                    (loop
                        br 0)))
            "#,
        );
        let registration = base_registration(&module, "on_user_prompt");

        let error = DefaultWasmHookExecutor
            .execute(
                &registration,
                HookContext {
                    event: HookEvent::UserPromptSubmit,
                    run_id: RunId::from("run_1"),
                    session_id: SessionId::from("session_1"),
                    turn_id: None,
                    fields: Default::default(),
                    payload: serde_json::json!({}),
                },
            )
            .await
            .unwrap_err();
        assert!(!error.to_string().is_empty());
    }
}
