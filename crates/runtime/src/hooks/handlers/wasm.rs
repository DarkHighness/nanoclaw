use super::execution::{
    HookAuditAction, HookExecutionObserver, TracingHookExecutionObserver, authorize_execute_path,
    record_completion, record_failure,
};
use crate::{Result, RuntimeError};
use async_trait::async_trait;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};
use tokio::sync::Mutex as AsyncMutex;
use tools::ToolExecutionContext;
use types::{
    HookContext, HookEffect, HookExecutionPolicy, HookHandler, HookHostApiGrant, HookRegistration,
    HookResult,
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

#[derive(Clone)]
pub struct DefaultWasmHookExecutor {
    module_cache: Arc<WasmModuleCache>,
    observer: Arc<dyn HookExecutionObserver>,
}

#[derive(Default)]
struct WasmModuleCache {
    runtimes: Mutex<HashMap<PathBuf, Arc<CachedWasmRuntime>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ModuleFingerprint {
    file_len: u64,
    modified_at_ms: u64,
}

struct CachedWasmRuntime {
    engine: Engine,
    module: Module,
    fingerprint: ModuleFingerprint,
    execution_lock: Arc<AsyncMutex<()>>,
}

#[async_trait]
impl WasmHookExecutor for DefaultWasmHookExecutor {
    async fn execute(
        &self,
        registration: &HookRegistration,
        context: HookContext,
    ) -> Result<HookResult> {
        let HookHandler::Wasm(wasm) = &registration.handler else {
            return Err(RuntimeError::hook(format!(
                "hook `{}` is not a wasm hook",
                registration.name
            )));
        };
        let module_path = PathBuf::from(&wasm.module);
        let authorized =
            authorize_execute_path(registration, "wasm", &module_path, self.observer.as_ref())?;
        let fingerprint = module_fingerprint(&module_path).map_err(|error| {
            record_failure(
                self.observer.as_ref(),
                registration,
                "wasm",
                HookAuditAction::ExecutePath,
                module_path.display().to_string(),
                &error,
            );
            error
        })?;
        let runtime = self
            .module_cache
            .get_or_load(module_path, fingerprint)
            .await
            .map_err(|error| {
                record_failure(
                    self.observer.as_ref(),
                    registration,
                    "wasm",
                    HookAuditAction::ExecutePath,
                    wasm.module.clone(),
                    &error,
                );
                error
            })?;
        let execution_guard = runtime.execution_lock.clone().lock_owned().await;
        let registration = registration.clone();
        let registration_for_call = registration.clone();
        let runtime_for_call = runtime.clone();
        let tool_context = authorized.tool_context.clone();
        let timeout_ms = registration.timeout_ms.unwrap_or(DEFAULT_WASM_TIMEOUT_MS);
        let watchdog_engine = runtime.engine.clone();
        let watchdog = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(timeout_ms)).await;
            watchdog_engine.increment_epoch();
        });
        let join_result = tokio::task::spawn_blocking(move || {
            let _execution_guard = execution_guard;
            execute_wasm_hook(
                &runtime_for_call,
                &registration_for_call,
                context,
                tool_context,
            )
        })
        .await;
        watchdog.abort();
        let result = join_result.map_err(|error| {
            let error = RuntimeError::hook(format!("wasm hook task failed: {error}"));
            record_failure(
                self.observer.as_ref(),
                &registration,
                "wasm",
                HookAuditAction::ExecutePath,
                wasm.module.clone(),
                &error,
            );
            error
        })?;
        match result {
            Ok(output) => {
                record_completion(
                    self.observer.as_ref(),
                    &registration,
                    "wasm",
                    HookAuditAction::ExecutePath,
                    wasm.module.clone(),
                );
                Ok(output)
            }
            Err(error) => {
                record_failure(
                    self.observer.as_ref(),
                    &registration,
                    "wasm",
                    HookAuditAction::ExecutePath,
                    wasm.module.clone(),
                    &error,
                );
                Err(error)
            }
        }
    }
}

struct WasmHostState {
    context_json: Vec<u8>,
    emitted_effects: Vec<HookEffect>,
    tool_context: ToolExecutionContext,
    execution: HookExecutionPolicy,
}

impl DefaultWasmHookExecutor {
    #[cfg(test)]
    fn with_observer(observer: Arc<dyn HookExecutionObserver>) -> Self {
        Self {
            module_cache: Arc::new(WasmModuleCache::default()),
            observer,
        }
    }

    #[cfg(test)]
    fn cached_module_count(&self) -> usize {
        self.module_cache.cached_module_count()
    }
}

impl Default for DefaultWasmHookExecutor {
    fn default() -> Self {
        Self {
            module_cache: Arc::new(WasmModuleCache::default()),
            observer: Arc::new(TracingHookExecutionObserver),
        }
    }
}

impl WasmModuleCache {
    async fn get_or_load(
        &self,
        module_path: PathBuf,
        fingerprint: ModuleFingerprint,
    ) -> Result<Arc<CachedWasmRuntime>> {
        if let Some(runtime) = self.get_if_fresh(&module_path, &fingerprint) {
            return Ok(runtime);
        }

        let module_path_for_load = module_path.clone();
        let loaded = tokio::task::spawn_blocking(move || {
            CachedWasmRuntime::load(module_path_for_load, fingerprint)
        })
        .await
        .map_err(|error| RuntimeError::hook(format!("wasm module load task failed: {error}")))??;

        let loaded = Arc::new(loaded);
        let mut cache = self
            .runtimes
            .lock()
            .map_err(|_| RuntimeError::invalid_state("wasm module cache lock poisoned"))?;
        match cache.get(&module_path) {
            Some(existing) if existing.fingerprint == loaded.fingerprint => Ok(existing.clone()),
            _ => {
                cache.insert(module_path, loaded.clone());
                Ok(loaded)
            }
        }
    }

    fn get_if_fresh(
        &self,
        module_path: &Path,
        fingerprint: &ModuleFingerprint,
    ) -> Option<Arc<CachedWasmRuntime>> {
        self.runtimes
            .lock()
            .ok()
            .and_then(|cache| cache.get(module_path).cloned())
            .filter(|runtime| runtime.fingerprint == *fingerprint)
    }

    #[cfg(test)]
    fn cached_module_count(&self) -> usize {
        self.runtimes.lock().map(|cache| cache.len()).unwrap_or(0)
    }
}

impl CachedWasmRuntime {
    fn load(module_path: PathBuf, fingerprint: ModuleFingerprint) -> Result<Self> {
        let engine = build_wasm_engine()?;
        let module = Module::from_file(&engine, &module_path)
            .map_err(|error| RuntimeError::hook(format!("failed to load wasm module: {error}")))?;
        Ok(Self {
            engine,
            module,
            fingerprint,
            execution_lock: Arc::new(AsyncMutex::new(())),
        })
    }
}

fn execute_wasm_hook(
    runtime: &CachedWasmRuntime,
    registration: &HookRegistration,
    context: HookContext,
    tool_context: ToolExecutionContext,
) -> Result<HookResult> {
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

    let mut linker = Linker::new(&runtime.engine);
    bind_host_functions(&mut linker)?;

    let mut store = Store::new(
        &runtime.engine,
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
        .instantiate(&mut store, &runtime.module)
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

fn build_wasm_engine() -> Result<Engine> {
    let mut config = Config::new();
    config.epoch_interruption(true);
    config.consume_fuel(true);
    Engine::new(&config)
        .map_err(|error| RuntimeError::hook(format!("failed to initialize wasm engine: {error}")))
}

fn module_fingerprint(module_path: &Path) -> Result<ModuleFingerprint> {
    let metadata = fs::metadata(module_path)
        .map_err(|error| RuntimeError::hook(format!("failed to inspect wasm module: {error}")))?;
    let modified_at_ms = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0);
    Ok(ModuleFingerprint {
        file_len: metadata.len(),
        modified_at_ms,
    })
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
    use crate::hooks::handlers::execution::{
        HookAuditAction, HookAuditEvent, HookAuditOutcome, HookExecutionObserver,
    };
    use std::sync::{Arc, Mutex};
    use types::{
        AgentSessionId, HookContext, HookEvent, HookExecutionPolicy, HookHandler, HookHostApiGrant,
        HookRegistration, HookResult, MessagePart, MessageRole, SessionId, WasmHookHandler,
    };

    #[derive(Default)]
    struct RecordingObserver {
        events: Mutex<Vec<HookAuditEvent>>,
    }

    impl HookExecutionObserver for RecordingObserver {
        fn record(&self, event: HookAuditEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

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
            name: "wasm".into(),
            event: HookEvent::UserPromptSubmit,
            matcher: None,
            handler: HookHandler::Wasm(WasmHookHandler {
                module: module.to_string_lossy().to_string(),
                entrypoint: entrypoint.to_string(),
            }),
            timeout_ms: Some(50),
            execution: Some(HookExecutionPolicy {
                plugin_id: Some("team-policy".into()),
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
            plugin_id: Some("team-policy".into()),
            plugin_root: Some(dir.path().to_path_buf()),
            exec_roots: vec![dir.path().to_path_buf()],
            host_api_grants: vec![
                HookHostApiGrant::GetHookContext,
                HookHostApiGrant::EmitHookEffect,
            ],
            ..HookExecutionPolicy::default()
        });

        let executor = DefaultWasmHookExecutor::default();
        let output = executor
            .execute(
                &registration,
                HookContext {
                    event: HookEvent::UserPromptSubmit,
                    session_id: SessionId::from("run_1"),
                    agent_session_id: AgentSessionId::from("session_1"),
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
            plugin_id: Some("team-policy".into()),
            plugin_root: Some(dir.path().to_path_buf()),
            exec_roots: vec![dir.path().to_path_buf()],
            host_api_grants: vec![HookHostApiGrant::ReadFile],
            ..HookExecutionPolicy::default()
        });

        let executor = DefaultWasmHookExecutor::default();
        let error = executor
            .execute(
                &registration,
                HookContext {
                    event: HookEvent::UserPromptSubmit,
                    session_id: SessionId::from("run_1"),
                    agent_session_id: AgentSessionId::from("session_1"),
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

        let executor = DefaultWasmHookExecutor::default();
        let error = executor
            .execute(
                &registration,
                HookContext {
                    event: HookEvent::UserPromptSubmit,
                    session_id: SessionId::from("run_1"),
                    agent_session_id: AgentSessionId::from("session_1"),
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
    async fn wasm_hook_reuses_cached_module_and_reloads_when_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let module_path = dir.path().join("cached.wasm");
        let make_module = |text: &str| {
            std::fs::write(
                &module_path,
                wat::parse_str(format!(
                    r#"(module
                        (import "{host}" "emit_effect" (func $emit_effect (param i32 i32) (result i32)))
                        (memory (export "memory") 1)
                        (data (i32.const 0) "{{\"kind\":\"append_message\",\"role\":\"system\",\"parts\":[{{\"type\":\"text\",\"text\":\"{text}\"}}]}}")
                        (func (export "on_user_prompt")
                            i32.const 0
                            i32.const {len}
                            call $emit_effect
                            drop))
                    "#,
                    host = super::HOST_MODULE,
                    text = wat_string(text),
                    len = format!(
                        "{{\"kind\":\"append_message\",\"role\":\"system\",\"parts\":[{{\"type\":\"text\",\"text\":\"{text}\"}}]}}",
                        text = text
                    )
                    .len(),
                ))
                .unwrap(),
            )
            .unwrap();
        };
        make_module("from cache one");

        let mut registration = base_registration(&module_path, "on_user_prompt");
        registration.execution = Some(HookExecutionPolicy {
            plugin_id: Some("team-policy".into()),
            plugin_root: Some(dir.path().to_path_buf()),
            exec_roots: vec![dir.path().to_path_buf()],
            host_api_grants: vec![HookHostApiGrant::EmitHookEffect],
            ..HookExecutionPolicy::default()
        });
        let executor = DefaultWasmHookExecutor::default();

        let first = executor
            .execute(
                &registration,
                HookContext {
                    event: HookEvent::UserPromptSubmit,
                    session_id: SessionId::from("run_1"),
                    agent_session_id: AgentSessionId::from("session_1"),
                    turn_id: None,
                    fields: Default::default(),
                    payload: serde_json::json!({}),
                },
            )
            .await
            .unwrap();
        assert_eq!(executor.cached_module_count(), 1);
        assert_eq!(
            first.effects,
            vec![types::HookEffect::AppendMessage {
                role: MessageRole::System,
                parts: vec![MessagePart::text("from cache one")],
            }]
        );

        let second = executor
            .execute(
                &registration,
                HookContext {
                    event: HookEvent::UserPromptSubmit,
                    session_id: SessionId::from("run_2"),
                    agent_session_id: AgentSessionId::from("session_2"),
                    turn_id: None,
                    fields: Default::default(),
                    payload: serde_json::json!({}),
                },
            )
            .await
            .unwrap();
        assert_eq!(executor.cached_module_count(), 1);
        assert_eq!(second.effects, first.effects);

        make_module("from cache two updated");
        let updated = executor
            .execute(
                &registration,
                HookContext {
                    event: HookEvent::UserPromptSubmit,
                    session_id: SessionId::from("run_3"),
                    agent_session_id: AgentSessionId::from("session_3"),
                    turn_id: None,
                    fields: Default::default(),
                    payload: serde_json::json!({}),
                },
            )
            .await
            .unwrap();
        assert_eq!(executor.cached_module_count(), 1);
        assert_eq!(
            updated.effects,
            vec![types::HookEffect::AppendMessage {
                role: MessageRole::System,
                parts: vec![MessagePart::text("from cache two updated")],
            }]
        );
    }

    #[tokio::test]
    async fn wasm_hook_uses_shared_audit_plane() {
        let dir = tempfile::tempdir().unwrap();
        let effect_json = "{\"kind\":\"append_message\",\"role\":\"system\",\"parts\":[{\"type\":\"text\",\"text\":\"from wasm\"}]}";
        let module = write_wasm_module(
            &dir,
            "ok.wasm",
            &format!(
                r#"(module
                    (import "{host}" "emit_effect" (func $emit_effect (param i32 i32) (result i32)))
                    (memory (export "memory") 1)
                    (data (i32.const 0) "{effect_data}")
                    (func (export "on_user_prompt")
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
            plugin_id: Some("team-policy".into()),
            plugin_root: Some(dir.path().to_path_buf()),
            exec_roots: vec![dir.path().to_path_buf()],
            host_api_grants: vec![HookHostApiGrant::EmitHookEffect],
            ..HookExecutionPolicy::default()
        });
        let observer = Arc::new(RecordingObserver::default());
        let executor = DefaultWasmHookExecutor::with_observer(observer.clone());

        executor
            .execute(
                &registration,
                HookContext {
                    event: HookEvent::UserPromptSubmit,
                    session_id: SessionId::from("run_1"),
                    agent_session_id: AgentSessionId::from("session_1"),
                    turn_id: None,
                    fields: Default::default(),
                    payload: serde_json::json!({}),
                },
            )
            .await
            .unwrap();

        let events = observer.events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].action, HookAuditAction::ExecutePath);
        assert_eq!(events[0].outcome, HookAuditOutcome::Allowed);
        assert_eq!(events[1].outcome, HookAuditOutcome::Completed);
    }
}
