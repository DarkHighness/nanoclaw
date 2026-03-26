use crate::code_intel::{
    CodeIntelBackend, CodeLocation, CodeNavigationTarget, CodeReference, CodeSymbol,
    CodeSymbolKind, WorkspaceTextCodeIntelBackend,
};
use crate::file_activity::FileActivityObserver;
use crate::process::{
    ExecRequest, ExecutionOrigin, ProcessExecutor, ProcessStdio, RuntimeScope, SandboxPolicy,
};
use crate::{Result, ToolError, ToolExecutionContext, stable_text_hash};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{Mutex as AsyncMutex, oneshot};
use tokio::time::timeout;
use tracing::{debug, info, warn};

const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(20);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Debug)]
pub struct ManagedCodeIntelOptions {
    pub enabled: bool,
    pub auto_install: bool,
    pub install_root: PathBuf,
}

impl ManagedCodeIntelOptions {
    #[must_use]
    pub fn for_workspace(workspace_root: &Path) -> Self {
        Self {
            enabled: true,
            auto_install: false,
            install_root: workspace_root.join(".agent-core/lsp"),
        }
    }
}

#[derive(Clone)]
pub struct ManagedCodeIntelBackend {
    runtime: Arc<ManagedLspRuntime>,
    fallback: WorkspaceTextCodeIntelBackend,
}

impl ManagedCodeIntelBackend {
    #[must_use]
    pub fn new(
        workspace_root: PathBuf,
        options: ManagedCodeIntelOptions,
        process_executor: Arc<dyn ProcessExecutor>,
        server_policy: SandboxPolicy,
        install_policy: SandboxPolicy,
    ) -> Self {
        Self {
            runtime: Arc::new(ManagedLspRuntime::new(
                workspace_root,
                options,
                process_executor,
                server_policy,
                install_policy,
            )),
            fallback: WorkspaceTextCodeIntelBackend::new(),
        }
    }
}

impl FileActivityObserver for ManagedCodeIntelBackend {
    fn did_open(&self, path: PathBuf) {
        self.runtime.spawn_sync(path, FileSyncEvent::Open);
    }

    fn did_change(&self, path: PathBuf) {
        self.runtime.spawn_sync(path, FileSyncEvent::Change);
    }

    fn did_remove(&self, path: PathBuf) {
        self.runtime.spawn_sync(path, FileSyncEvent::Remove);
    }
}

#[async_trait]
impl CodeIntelBackend for ManagedCodeIntelBackend {
    fn name(&self) -> &'static str {
        "managed_lsp_with_text_fallback_v1"
    }

    async fn workspace_symbols(
        &self,
        query: &str,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> Result<Vec<CodeSymbol>> {
        let semantic = self.runtime.workspace_symbols(query, limit).await?;
        let lexical = self.fallback.workspace_symbols(query, limit, ctx).await?;
        Ok(merge_symbols(semantic, lexical, limit))
    }

    async fn document_symbols(
        &self,
        path: &Path,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> Result<Vec<CodeSymbol>> {
        let semantic = self.runtime.document_symbols(path, limit).await?;
        if !semantic.is_empty() {
            return Ok(semantic);
        }
        self.fallback.document_symbols(path, limit, ctx).await
    }

    async fn definitions(
        &self,
        target: &CodeNavigationTarget,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> Result<Vec<CodeSymbol>> {
        let semantic = self.runtime.definitions(target, limit).await?;
        if !semantic.is_empty() {
            return Ok(semantic);
        }
        self.fallback.definitions(target, limit, ctx).await
    }

    async fn references(
        &self,
        target: &CodeNavigationTarget,
        include_declaration: bool,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> Result<Vec<CodeReference>> {
        let semantic = self
            .runtime
            .references(target, include_declaration, limit)
            .await?;
        if !semantic.is_empty() {
            return Ok(semantic);
        }
        self.fallback
            .references(target, include_declaration, limit, ctx)
            .await
    }
}

struct ManagedLspRuntime {
    workspace_root: PathBuf,
    options: ManagedCodeIntelOptions,
    process_executor: Arc<dyn ProcessExecutor>,
    server_policy: SandboxPolicy,
    install_policy: SandboxPolicy,
    slots: Mutex<BTreeMap<&'static str, Arc<SessionSlot>>>,
    logged_unavailable: Mutex<BTreeSet<&'static str>>,
}

impl ManagedLspRuntime {
    fn new(
        workspace_root: PathBuf,
        options: ManagedCodeIntelOptions,
        process_executor: Arc<dyn ProcessExecutor>,
        server_policy: SandboxPolicy,
        install_policy: SandboxPolicy,
    ) -> Self {
        Self {
            workspace_root,
            options,
            process_executor,
            server_policy,
            install_policy,
            slots: Mutex::new(BTreeMap::new()),
            logged_unavailable: Mutex::new(BTreeSet::new()),
        }
    }

    fn spawn_sync(self: &Arc<Self>, path: PathBuf, event: FileSyncEvent) {
        if !self.options.enabled {
            return;
        }
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(error) = runtime.handle_file_event(path.clone(), event).await {
                debug!(
                    "background LSP sync skipped for {}: {error}",
                    path.display()
                );
            }
        });
    }

    async fn handle_file_event(&self, path: PathBuf, event: FileSyncEvent) -> Result<()> {
        match event {
            FileSyncEvent::Open | FileSyncEvent::Change => {
                self.ensure_document_synced(&path).await?;
            }
            FileSyncEvent::Remove => {
                self.close_document(&path).await?;
            }
        }
        Ok(())
    }

    async fn workspace_symbols(&self, query: &str, limit: usize) -> Result<Vec<CodeSymbol>> {
        if !self.options.enabled {
            return Ok(Vec::new());
        }

        let mut symbols = Vec::new();
        for session in self.ready_sessions() {
            let response = session
                .request("workspace/symbol", json!({ "query": query }))
                .await;
            let Ok(response) = response else {
                continue;
            };
            symbols.extend(parse_workspace_symbols(&response, &self.workspace_root));
            if symbols.len() >= limit {
                break;
            }
        }
        symbols.truncate(limit);
        Ok(symbols)
    }

    async fn document_symbols(&self, path: &Path, limit: usize) -> Result<Vec<CodeSymbol>> {
        let Some(session) = self.ensure_session_for_path(path).await? else {
            return Ok(Vec::new());
        };
        self.ensure_document_synced(path).await?;
        let uri = file_uri_from_path(path);
        let response = session
            .request(
                "textDocument/documentSymbol",
                json!({ "textDocument": { "uri": uri } }),
            )
            .await?;
        let mut symbols = parse_document_symbols(&response, &self.workspace_root, path);
        symbols.truncate(limit);
        Ok(symbols)
    }

    async fn definitions(
        &self,
        target: &CodeNavigationTarget,
        limit: usize,
    ) -> Result<Vec<CodeSymbol>> {
        match target {
            CodeNavigationTarget::Position {
                path, line, column, ..
            } => {
                let Some(session) = self.ensure_session_for_path(path).await? else {
                    return Ok(Vec::new());
                };
                self.ensure_document_synced(path).await?;
                let symbol_name = symbol_name_for_target(target).unwrap_or_else(|| "symbol".into());
                let response = session
                    .request(
                        "textDocument/definition",
                        json!({
                            "textDocument": { "uri": file_uri_from_path(path) },
                            "position": zero_based_position(*line, *column),
                        }),
                    )
                    .await?;
                let mut symbols = parse_locations_as_symbols(
                    &response,
                    &self.workspace_root,
                    &symbol_name,
                    CodeSymbolKind::Unknown,
                );
                symbols.truncate(limit);
                Ok(symbols)
            }
            CodeNavigationTarget::Symbol(symbol) => {
                let query = symbol.trim();
                if query.is_empty() {
                    return Ok(Vec::new());
                }
                let mut matches = self.workspace_symbols(query, limit).await?;
                matches
                    .retain(|entry| entry.name == query || entry.name.eq_ignore_ascii_case(query));
                matches.truncate(limit);
                Ok(matches)
            }
        }
    }

    async fn references(
        &self,
        target: &CodeNavigationTarget,
        include_declaration: bool,
        limit: usize,
    ) -> Result<Vec<CodeReference>> {
        let CodeNavigationTarget::Position {
            path, line, column, ..
        } = target
        else {
            return Ok(Vec::new());
        };

        let Some(session) = self.ensure_session_for_path(path).await? else {
            return Ok(Vec::new());
        };
        self.ensure_document_synced(path).await?;
        let symbol_name = symbol_name_for_target(target).unwrap_or_else(|| "symbol".into());
        let response = session
            .request(
                "textDocument/references",
                json!({
                    "textDocument": { "uri": file_uri_from_path(path) },
                    "position": zero_based_position(*line, *column),
                    "context": { "includeDeclaration": include_declaration },
                }),
            )
            .await?;
        let mut references =
            parse_locations_as_references(&response, &self.workspace_root, &symbol_name).await;
        references.truncate(limit);
        Ok(references)
    }

    fn ready_sessions(&self) -> Vec<Arc<LspSession>> {
        let slots = self.slots.lock().expect("LSP session slots lock");
        slots
            .values()
            .filter_map(|slot| {
                slot.state
                    .try_lock()
                    .ok()
                    .and_then(|state| state.as_ref().cloned())
            })
            .collect()
    }

    async fn ensure_session_for_path(&self, path: &Path) -> Result<Option<Arc<LspSession>>> {
        if !self.options.enabled {
            return Ok(None);
        }
        let Some(spec) = server_spec_for_path(path) else {
            return Ok(None);
        };
        let slot = {
            let mut slots = self.slots.lock().expect("LSP session slots lock");
            slots
                .entry(spec.id)
                .or_insert_with(|| Arc::new(SessionSlot::default()))
                .clone()
        };

        // One async mutex per language server prevents duplicate installs or process spawns
        // when multiple file-open hooks and semantic queries race the same server.
        let mut state = slot.state.lock().await;
        if let Some(session) = state.as_ref() {
            return Ok(Some(session.clone()));
        }

        let command = match self.resolve_command(spec).await {
            Ok(command) => command,
            Err(error) => {
                self.log_unavailable_once(spec, &error.to_string());
                return Ok(None);
            }
        };
        let session = Arc::new(
            LspSession::start(
                spec,
                self.workspace_root.clone(),
                command,
                self.process_executor.clone(),
                self.server_policy.clone(),
            )
            .await?,
        );
        *state = Some(session.clone());
        Ok(Some(session))
    }

    async fn ensure_document_synced(&self, path: &Path) -> Result<()> {
        let Some(session) = self.ensure_session_for_path(path).await? else {
            return Ok(());
        };
        let Some(language_id) = language_id_for_path(path) else {
            return Ok(());
        };
        let content = fs::read_to_string(path).await.map_err(|error| {
            ToolError::invalid_state(format!(
                "failed to read {} for LSP sync: {error}",
                path.display()
            ))
        })?;
        session.sync_document(path, language_id, content).await
    }

    async fn close_document(&self, path: &Path) -> Result<()> {
        let Some(spec) = server_spec_for_path(path) else {
            return Ok(());
        };
        let slot = {
            let slots = self.slots.lock().expect("LSP session slots lock");
            slots.get(spec.id).cloned()
        };
        let Some(slot) = slot else {
            return Ok(());
        };
        let state = slot.state.lock().await;
        if let Some(session) = state.as_ref() {
            session.close_document(path).await?;
        }
        Ok(())
    }

    async fn resolve_command(&self, spec: &'static LanguageServerSpec) -> Result<ResolvedCommand> {
        if let Some(command) = resolve_existing_command(spec, &self.options.install_root) {
            return Ok(command);
        }
        if !self.options.auto_install {
            return Err(ToolError::invalid_state(format!(
                "no managed LSP command found for {} and auto-install is disabled",
                spec.id
            )));
        }
        self.install_server(spec).await?;
        resolve_existing_command(spec, &self.options.install_root).ok_or_else(|| {
            ToolError::invalid_state(format!(
                "installed LSP server for {} but could not resolve its executable",
                spec.id
            ))
        })
    }

    async fn install_server(&self, spec: &'static LanguageServerSpec) -> Result<()> {
        let Some(install) = spec.install else {
            return Err(ToolError::invalid_state(format!(
                "{} does not support managed installation",
                spec.id
            )));
        };

        let server_root = self.options.install_root.join(spec.install_id);
        std::fs::create_dir_all(&server_root).map_err(|error| {
            ToolError::invalid_state(format!(
                "failed to create managed LSP install root {}: {error}",
                server_root.display()
            ))
        })?;

        let request = match install {
            InstallStrategy::Npm { packages } => ExecRequest {
                program: find_executable("npm")
                    .ok_or_else(|| {
                        ToolError::invalid_state(
                            "npm is required for managed npm-based LSP installs",
                        )
                    })?
                    .display()
                    .to_string(),
                args: build_npm_install_args(&server_root, packages),
                cwd: Some(server_root.clone()),
                env: BTreeMap::new(),
                stdin: ProcessStdio::Null,
                stdout: ProcessStdio::Piped,
                stderr: ProcessStdio::Piped,
                kill_on_drop: true,
                origin: ExecutionOrigin::HostUtility {
                    name: format!("lsp-install-{}", spec.id),
                },
                runtime_scope: RuntimeScope::default(),
                sandbox_policy: self.install_policy.clone(),
            },
            InstallStrategy::Go { module } => {
                let mut env = BTreeMap::new();
                let gobin = server_root.join("bin");
                std::fs::create_dir_all(&gobin).map_err(|error| {
                    ToolError::invalid_state(format!(
                        "failed to create managed go LSP bin dir {}: {error}",
                        gobin.display()
                    ))
                })?;
                env.insert("GOBIN".to_string(), gobin.display().to_string());
                ExecRequest {
                    program: find_executable("go")
                        .ok_or_else(|| {
                            ToolError::invalid_state(
                                "go is required for managed gopls installation",
                            )
                        })?
                        .display()
                        .to_string(),
                    args: vec!["install".to_string(), module.to_string()],
                    cwd: Some(server_root.clone()),
                    env,
                    stdin: ProcessStdio::Null,
                    stdout: ProcessStdio::Piped,
                    stderr: ProcessStdio::Piped,
                    kill_on_drop: true,
                    origin: ExecutionOrigin::HostUtility {
                        name: format!("lsp-install-{}", spec.id),
                    },
                    runtime_scope: RuntimeScope::default(),
                    sandbox_policy: self.install_policy.clone(),
                }
            }
            InstallStrategy::Pip { packages } => {
                let python = find_executable("python3")
                    .or_else(|| find_executable("python"))
                    .ok_or_else(|| {
                        ToolError::invalid_state(
                            "python3 or python is required for managed Python LSP installation",
                        )
                    })?;
                ExecRequest {
                    program: python.display().to_string(),
                    args: build_pip_install_args(&server_root, packages),
                    cwd: Some(server_root.clone()),
                    env: BTreeMap::new(),
                    stdin: ProcessStdio::Null,
                    stdout: ProcessStdio::Piped,
                    stderr: ProcessStdio::Piped,
                    kill_on_drop: true,
                    origin: ExecutionOrigin::HostUtility {
                        name: format!("lsp-install-{}", spec.id),
                    },
                    runtime_scope: RuntimeScope::default(),
                    sandbox_policy: self.install_policy.clone(),
                }
            }
            InstallStrategy::Cargo { package } => ExecRequest {
                program: find_executable("cargo")
                    .ok_or_else(|| {
                        ToolError::invalid_state(
                            "cargo is required for managed cargo-based LSP installs",
                        )
                    })?
                    .display()
                    .to_string(),
                args: build_cargo_install_args(&server_root, package),
                cwd: Some(server_root.clone()),
                env: BTreeMap::new(),
                stdin: ProcessStdio::Null,
                stdout: ProcessStdio::Piped,
                stderr: ProcessStdio::Piped,
                kill_on_drop: true,
                origin: ExecutionOrigin::HostUtility {
                    name: format!("lsp-install-{}", spec.id),
                },
                runtime_scope: RuntimeScope::default(),
                sandbox_policy: self.install_policy.clone(),
            },
        };

        info!(
            "installing managed LSP server {} into {}",
            spec.id,
            server_root.display()
        );
        let mut command = self.process_executor.prepare(request)?;
        let output = timeout(INSTALL_TIMEOUT, command.output())
            .await
            .map_err(|_| {
                ToolError::invalid_state(format!("timed out installing managed LSP {}", spec.id))
            })?
            .map_err(|error| {
                ToolError::invalid_state(format!(
                    "failed to execute managed LSP installer for {}: {error}",
                    spec.id
                ))
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let detail = if !stderr.is_empty() { stderr } else { stdout };
            return Err(ToolError::invalid_state(format!(
                "managed LSP install for {} failed: {detail}",
                spec.id
            )));
        }
        Ok(())
    }

    fn log_unavailable_once(&self, spec: &'static LanguageServerSpec, message: &str) {
        let mut logged = self
            .logged_unavailable
            .lock()
            .expect("LSP unavailable set lock");
        if logged.insert(spec.id) {
            warn!("managed LSP {} unavailable: {message}", spec.id);
        }
    }
}

#[derive(Clone, Copy)]
enum FileSyncEvent {
    Open,
    Change,
    Remove,
}

#[derive(Default)]
struct SessionSlot {
    state: AsyncMutex<Option<Arc<LspSession>>>,
}

struct LspSession {
    spec: &'static LanguageServerSpec,
    workspace_root: PathBuf,
    child: Mutex<Child>,
    stdin: AsyncMutex<ChildStdin>,
    next_id: AtomicI64,
    pending: Arc<Mutex<BTreeMap<i64, oneshot::Sender<std::result::Result<Value, String>>>>>,
    documents: Mutex<BTreeMap<PathBuf, OpenDocument>>,
}

impl LspSession {
    async fn start(
        spec: &'static LanguageServerSpec,
        workspace_root: PathBuf,
        command: ResolvedCommand,
        process_executor: Arc<dyn ProcessExecutor>,
        sandbox_policy: SandboxPolicy,
    ) -> Result<Self> {
        let request = ExecRequest {
            program: command.program.display().to_string(),
            args: command.args,
            cwd: Some(workspace_root.clone()),
            env: command.env,
            stdin: ProcessStdio::Piped,
            stdout: ProcessStdio::Piped,
            stderr: ProcessStdio::Piped,
            kill_on_drop: true,
            origin: ExecutionOrigin::HostUtility {
                name: format!("lsp-server-{}", spec.id),
            },
            runtime_scope: RuntimeScope::default(),
            sandbox_policy,
        };
        let mut child = process_executor
            .prepare(request)?
            .spawn()
            .map_err(|error| {
                ToolError::invalid_state(format!(
                    "failed to start managed LSP server {}: {error}",
                    spec.id
                ))
            })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            ToolError::invalid_state(format!("LSP server {} did not expose stdin", spec.id))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            ToolError::invalid_state(format!("LSP server {} did not expose stdout", spec.id))
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            ToolError::invalid_state(format!("LSP server {} did not expose stderr", spec.id))
        })?;

        let session = Self {
            spec,
            workspace_root: workspace_root.clone(),
            child: Mutex::new(child),
            stdin: AsyncMutex::new(stdin),
            next_id: AtomicI64::new(1),
            pending: Arc::new(Mutex::new(BTreeMap::new())),
            documents: Mutex::new(BTreeMap::new()),
        };
        session.spawn_reader(stdout);
        session.spawn_stderr(stderr);
        session.initialize().await?;
        Ok(session)
    }

    fn spawn_reader(&self, stdout: tokio::process::ChildStdout) {
        let pending = Arc::clone(&self.pending);
        let server_id = self.spec.id;
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                let message = read_lsp_message(&mut reader).await;
                let Some(message) = message.transpose() else {
                    break;
                };
                let Ok(message) = message else {
                    warn!("managed LSP reader for {} stopped with error", server_id);
                    break;
                };
                if let Some(id) = message.get("id").and_then(Value::as_i64) {
                    let sender = pending.lock().expect("LSP pending map lock").remove(&id);
                    if let Some(sender) = sender {
                        let response = if let Some(error) = message.get("error") {
                            Err(error.to_string())
                        } else {
                            Ok(message.get("result").cloned().unwrap_or(Value::Null))
                        };
                        let _ = sender.send(response);
                    }
                }
            }
        });
    }

    fn spawn_stderr(&self, stderr: tokio::process::ChildStderr) {
        let server_id = self.spec.id;
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(_) => debug!("LSP stderr [{}] {}", server_id, line.trim_end()),
                    Err(_) => break,
                }
            }
        });
    }

    async fn initialize(&self) -> Result<()> {
        let root_uri = file_uri_from_path(&self.workspace_root);
        let root_name = self
            .workspace_root
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("workspace");
        let result = timeout(
            INITIALIZE_TIMEOUT,
            self.request(
                "initialize",
                json!({
                    "processId": std::process::id(),
                    "rootUri": root_uri,
                    "rootPath": self.workspace_root.display().to_string(),
                    "workspaceFolders": [{ "uri": root_uri, "name": root_name }],
                    "capabilities": {
                        "workspace": { "symbol": {} },
                        "textDocument": {
                            "definition": { "linkSupport": true },
                            "references": {},
                            "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                            "synchronization": { "didSave": true },
                        }
                    }
                }),
            ),
        )
        .await
        .map_err(|_| {
            ToolError::invalid_state(format!(
                "timed out initializing managed LSP server {}",
                self.spec.id
            ))
        })??;
        let _ = result;
        self.notify("initialized", json!({})).await?;
        info!("managed LSP server {} initialized", self.spec.id);
        Ok(())
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .expect("LSP pending map lock")
            .insert(id, tx);
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        if let Err(error) = self.write_message(&message).await {
            self.pending
                .lock()
                .expect("LSP pending map lock")
                .remove(&id);
            return Err(error);
        }
        timeout(REQUEST_TIMEOUT, rx)
            .await
            .map_err(|_| {
                ToolError::invalid_state(format!(
                    "timed out waiting for LSP {} response to {}",
                    self.spec.id, method
                ))
            })?
            .map_err(|_| {
                ToolError::invalid_state(format!(
                    "managed LSP {} dropped the response channel for {}",
                    self.spec.id, method
                ))
            })?
            .map_err(|error| ToolError::invalid_state(error))
    }

    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
        .await
    }

    async fn write_message(&self, message: &Value) -> Result<()> {
        let body = serde_json::to_vec(message).map_err(|error| {
            ToolError::invalid_state(format!("failed to serialize LSP payload: {error}"))
        })?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(header.as_bytes()).await.map_err(|error| {
            ToolError::invalid_state(format!(
                "failed to write LSP header to {}: {error}",
                self.spec.id
            ))
        })?;
        stdin.write_all(&body).await.map_err(|error| {
            ToolError::invalid_state(format!(
                "failed to write LSP body to {}: {error}",
                self.spec.id
            ))
        })?;
        stdin.flush().await.map_err(|error| {
            ToolError::invalid_state(format!(
                "failed to flush LSP payload to {}: {error}",
                self.spec.id
            ))
        })
    }

    async fn sync_document(
        &self,
        path: &Path,
        language_id: &'static str,
        content: String,
    ) -> Result<()> {
        let snapshot = stable_text_hash(&content);
        let existing = self
            .documents
            .lock()
            .expect("LSP document map lock")
            .get(path)
            .cloned();
        match existing {
            Some(document) if document.snapshot == snapshot => Ok(()),
            Some(document) => {
                let next_version = document.version + 1;
                self.notify(
                    "textDocument/didChange",
                    json!({
                        "textDocument": {
                            "uri": file_uri_from_path(path),
                            "version": next_version,
                        },
                        "contentChanges": [{ "text": content }],
                    }),
                )
                .await?;
                self.documents
                    .lock()
                    .expect("LSP document map lock")
                    .insert(
                        path.to_path_buf(),
                        OpenDocument {
                            version: next_version,
                            snapshot,
                        },
                    );
                Ok(())
            }
            None => {
                self.notify(
                    "textDocument/didOpen",
                    json!({
                        "textDocument": {
                            "uri": file_uri_from_path(path),
                            "languageId": language_id,
                            "version": 1,
                            "text": content,
                        }
                    }),
                )
                .await?;
                self.documents
                    .lock()
                    .expect("LSP document map lock")
                    .insert(
                        path.to_path_buf(),
                        OpenDocument {
                            version: 1,
                            snapshot,
                        },
                    );
                Ok(())
            }
        }
    }

    async fn close_document(&self, path: &Path) -> Result<()> {
        let removed = self
            .documents
            .lock()
            .expect("LSP document map lock")
            .remove(path);
        if removed.is_none() {
            return Ok(());
        }
        self.notify(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": file_uri_from_path(path) } }),
        )
        .await
    }
}

impl Drop for LspSession {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.start_kill();
        }
    }
}

#[derive(Clone)]
struct OpenDocument {
    version: i32,
    snapshot: String,
}

#[derive(Clone)]
struct ResolvedCommand {
    program: PathBuf,
    args: Vec<String>,
    env: BTreeMap<String, String>,
}

#[derive(Clone, Copy)]
struct LanguageServerSpec {
    id: &'static str,
    install_id: &'static str,
    command: &'static str,
    args: &'static [&'static str],
    install: Option<InstallStrategy>,
}

#[derive(Clone, Copy)]
struct LanguageSupport {
    language_id: &'static str,
    server: &'static LanguageServerSpec,
    extensions: &'static [&'static str],
    file_names: &'static [&'static str],
    file_name_prefixes: &'static [&'static str],
}

impl LanguageSupport {
    fn matches(&self, signature: &PathSignature) -> bool {
        signature.extension.as_deref().is_some_and(|extension| {
            self.extensions
                .iter()
                .any(|candidate| *candidate == extension)
        }) || signature.file_name.as_deref().is_some_and(|file_name| {
            self.file_names
                .iter()
                .any(|candidate| *candidate == file_name)
                || self
                    .file_name_prefixes
                    .iter()
                    .any(|candidate| file_name.starts_with(candidate))
        })
    }
}

#[derive(Default)]
struct PathSignature {
    extension: Option<String>,
    file_name: Option<String>,
}

impl PathSignature {
    fn from_path(path: &Path) -> Self {
        Self {
            extension: lowercase_extension(path),
            file_name: lowercase_file_name(path),
        }
    }
}

#[derive(Clone, Copy)]
enum InstallStrategy {
    Npm { packages: &'static [&'static str] },
    Go { module: &'static str },
    Pip { packages: &'static [&'static str] },
    Cargo { package: &'static str },
}

const TYPESCRIPT_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "typescript",
    install_id: "typescript",
    command: "typescript-language-server",
    args: &["--stdio"],
    install: Some(InstallStrategy::Npm {
        packages: &["typescript", "typescript-language-server"],
    }),
};

const HTML_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "html",
    install_id: "html",
    command: "vscode-html-language-server",
    args: &["--stdio"],
    install: Some(InstallStrategy::Npm {
        packages: &["vscode-langservers-extracted"],
    }),
};

const CSS_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "css",
    install_id: "css",
    command: "vscode-css-language-server",
    args: &["--stdio"],
    install: Some(InstallStrategy::Npm {
        packages: &["vscode-langservers-extracted"],
    }),
};

const JSON_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "json",
    install_id: "json",
    command: "vscode-json-language-server",
    args: &["--stdio"],
    install: Some(InstallStrategy::Npm {
        packages: &["vscode-langservers-extracted"],
    }),
};

const PYTHON_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "python",
    install_id: "python",
    command: "pylsp",
    args: &[],
    install: Some(InstallStrategy::Pip {
        packages: &["python-lsp-server"],
    }),
};

const GO_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "go",
    install_id: "go",
    command: "gopls",
    args: &[],
    install: Some(InstallStrategy::Go {
        module: "golang.org/x/tools/gopls@latest",
    }),
};

const YAML_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "yaml",
    install_id: "yaml",
    command: "yaml-language-server",
    args: &["--stdio"],
    install: Some(InstallStrategy::Npm {
        packages: &["yaml-language-server"],
    }),
};

const SHELL_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "shell",
    install_id: "shell",
    command: "bash-language-server",
    args: &["start"],
    install: Some(InstallStrategy::Npm {
        packages: &["bash-language-server"],
    }),
};

const DOCKERFILE_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "dockerfile",
    install_id: "dockerfile",
    command: "docker-langserver",
    args: &["--stdio"],
    install: Some(InstallStrategy::Npm {
        packages: &["dockerfile-language-server-nodejs"],
    }),
};

const PHP_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "php",
    install_id: "php",
    command: "intelephense",
    args: &["--stdio"],
    install: Some(InstallStrategy::Npm {
        packages: &["intelephense"],
    }),
};

const TOML_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "toml",
    install_id: "toml",
    command: "taplo",
    args: &["lsp", "stdio"],
    install: Some(InstallStrategy::Cargo {
        package: "taplo-cli",
    }),
};

const SQL_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "sql",
    install_id: "sql",
    command: "sqls",
    args: &[],
    install: Some(InstallStrategy::Go {
        module: "github.com/sqls-server/sqls@latest",
    }),
};

const RUST_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "rust",
    install_id: "rust",
    command: "rust-analyzer",
    args: &[],
    install: None,
};

const JAVA_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "java",
    install_id: "java",
    command: "jdtls",
    args: &[],
    install: None,
};

const CLANGD_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "clangd",
    install_id: "clangd",
    command: "clangd",
    args: &[],
    install: None,
};

// Language recognition is intentionally separate from server installation and launching.
// Several language ids can share one server binary, while install support remains a property
// of the server itself rather than of the file-extension matching layer.
const SUPPORTED_LANGUAGES: &[LanguageSupport] = &[
    LanguageSupport {
        language_id: "typescript",
        server: &TYPESCRIPT_SPEC,
        extensions: &["ts", "mts", "cts"],
        file_names: &[],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "typescriptreact",
        server: &TYPESCRIPT_SPEC,
        extensions: &["tsx"],
        file_names: &[],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "javascript",
        server: &TYPESCRIPT_SPEC,
        extensions: &["js", "mjs", "cjs"],
        file_names: &[],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "javascriptreact",
        server: &TYPESCRIPT_SPEC,
        extensions: &["jsx"],
        file_names: &[],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "html",
        server: &HTML_SPEC,
        extensions: &["html", "htm", "xhtml"],
        file_names: &[],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "css",
        server: &CSS_SPEC,
        extensions: &["css"],
        file_names: &[],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "scss",
        server: &CSS_SPEC,
        extensions: &["scss"],
        file_names: &[],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "sass",
        server: &CSS_SPEC,
        extensions: &["sass"],
        file_names: &[],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "less",
        server: &CSS_SPEC,
        extensions: &["less"],
        file_names: &[],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "json",
        server: &JSON_SPEC,
        extensions: &["json"],
        file_names: &[],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "jsonc",
        server: &JSON_SPEC,
        extensions: &["jsonc", "code-workspace"],
        file_names: &[],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "python",
        server: &PYTHON_SPEC,
        extensions: &["py", "pyi"],
        file_names: &[],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "go",
        server: &GO_SPEC,
        extensions: &["go"],
        file_names: &[],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "go.mod",
        server: &GO_SPEC,
        extensions: &[],
        file_names: &["go.mod"],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "go.sum",
        server: &GO_SPEC,
        extensions: &[],
        file_names: &["go.sum"],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "go.work",
        server: &GO_SPEC,
        extensions: &[],
        file_names: &["go.work"],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "yaml",
        server: &YAML_SPEC,
        extensions: &["yaml", "yml"],
        file_names: &[],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "shellscript",
        server: &SHELL_SPEC,
        extensions: &["sh", "bash", "zsh", "ksh"],
        file_names: &[
            ".bashrc",
            ".bash_profile",
            ".profile",
            ".zshrc",
            ".zprofile",
            ".kshrc",
        ],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "dockerfile",
        server: &DOCKERFILE_SPEC,
        extensions: &["dockerfile"],
        file_names: &["dockerfile", "containerfile"],
        file_name_prefixes: &["dockerfile.", "containerfile."],
    },
    LanguageSupport {
        language_id: "php",
        server: &PHP_SPEC,
        extensions: &["php", "phtml"],
        file_names: &[],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "toml",
        server: &TOML_SPEC,
        extensions: &["toml"],
        file_names: &[],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "sql",
        server: &SQL_SPEC,
        extensions: &["sql"],
        file_names: &[],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "rust",
        server: &RUST_SPEC,
        extensions: &["rs"],
        file_names: &[],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "java",
        server: &JAVA_SPEC,
        extensions: &["java"],
        file_names: &[],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "c",
        server: &CLANGD_SPEC,
        extensions: &["c", "h"],
        file_names: &[],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "cpp",
        server: &CLANGD_SPEC,
        extensions: &["cc", "cpp", "cxx", "c++", "hpp", "hh", "hxx"],
        file_names: &[],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "objective-c",
        server: &CLANGD_SPEC,
        extensions: &["m"],
        file_names: &[],
        file_name_prefixes: &[],
    },
    LanguageSupport {
        language_id: "objective-cpp",
        server: &CLANGD_SPEC,
        extensions: &["mm"],
        file_names: &[],
        file_name_prefixes: &[],
    },
];

fn language_support_for_path(path: &Path) -> Option<&'static LanguageSupport> {
    let signature = PathSignature::from_path(path);
    SUPPORTED_LANGUAGES
        .iter()
        .find(|support| support.matches(&signature))
}

fn server_spec_for_path(path: &Path) -> Option<&'static LanguageServerSpec> {
    language_support_for_path(path).map(|support| support.server)
}

fn language_id_for_path(path: &Path) -> Option<&'static str> {
    language_support_for_path(path).map(|support| support.language_id)
}

fn resolve_existing_command(
    spec: &'static LanguageServerSpec,
    install_root: &Path,
) -> Option<ResolvedCommand> {
    let managed = managed_executable_path(install_root, spec);
    if managed.is_file() {
        return Some(ResolvedCommand {
            program: managed,
            args: spec.args.iter().map(|value| (*value).to_string()).collect(),
            env: BTreeMap::new(),
        });
    }
    find_executable(spec.command).map(|program| ResolvedCommand {
        program,
        args: spec.args.iter().map(|value| (*value).to_string()).collect(),
        env: BTreeMap::new(),
    })
}

fn managed_executable_path(install_root: &Path, spec: &'static LanguageServerSpec) -> PathBuf {
    let server_root = install_root.join(spec.install_id);
    match spec.install {
        Some(InstallStrategy::Npm { .. }) => server_root
            .join("node_modules")
            .join(".bin")
            .join(spec.command),
        Some(InstallStrategy::Go { .. })
        | Some(InstallStrategy::Pip { .. })
        | Some(InstallStrategy::Cargo { .. }) => server_root.join("bin").join(spec.command),
        None => server_root.join(spec.command),
    }
}

fn build_npm_install_args(server_root: &Path, packages: &[&str]) -> Vec<String> {
    let mut args = vec![
        "install".to_string(),
        "--prefix".to_string(),
        server_root.display().to_string(),
        "--no-save".to_string(),
    ];
    args.extend(packages.iter().map(|value| (*value).to_string()));
    args
}

fn build_pip_install_args(server_root: &Path, packages: &[&str]) -> Vec<String> {
    let mut args = vec![
        "-m".to_string(),
        "pip".to_string(),
        "install".to_string(),
        "--upgrade".to_string(),
        "--prefix".to_string(),
        server_root.display().to_string(),
    ];
    args.extend(packages.iter().map(|value| (*value).to_string()));
    args
}

fn build_cargo_install_args(server_root: &Path, package: &str) -> Vec<String> {
    vec![
        "install".to_string(),
        "--root".to_string(),
        server_root.display().to_string(),
        "--locked".to_string(),
        package.to_string(),
    ]
}

fn lowercase_extension(path: &Path) -> Option<String> {
    path.extension()?.to_str().map(str::to_ascii_lowercase)
}

fn lowercase_file_name(path: &Path) -> Option<String> {
    path.file_name()?.to_str().map(str::to_ascii_lowercase)
}

fn find_executable(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var).find_map(|dir| {
        let candidate = dir.join(name);
        candidate.is_file().then_some(candidate)
    })
}

async fn read_lsp_message(
    reader: &mut BufReader<tokio::process::ChildStdout>,
) -> std::io::Result<Option<Value>> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).await?;
        if read == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = value.trim().parse::<usize>().ok();
        }
    }

    let Some(content_length) = content_length else {
        return Ok(None);
    };
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body).await?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(std::io::Error::other)
}

fn parse_workspace_symbols(value: &Value, workspace_root: &Path) -> Vec<CodeSymbol> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let name = entry.get("name")?.as_str()?.to_string();
            let location = parse_location_like(entry.get("location")?, workspace_root)?;
            Some(CodeSymbol {
                name,
                kind: parse_symbol_kind(entry.get("kind")),
                location,
                signature: entry
                    .get("detail")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
            })
        })
        .collect()
}

fn parse_document_symbols(
    value: &Value,
    workspace_root: &Path,
    document_path: &Path,
) -> Vec<CodeSymbol> {
    let mut symbols = Vec::new();
    let Some(entries) = value.as_array() else {
        return symbols;
    };
    for entry in entries {
        collect_document_symbols(entry, workspace_root, document_path, &mut symbols);
    }
    symbols.sort_by(|left, right| {
        (
            left.location.path.as_str(),
            left.location.line,
            left.location.column,
            left.name.as_str(),
        )
            .cmp(&(
                right.location.path.as_str(),
                right.location.line,
                right.location.column,
                right.name.as_str(),
            ))
    });
    symbols
}

fn collect_document_symbols(
    entry: &Value,
    workspace_root: &Path,
    document_path: &Path,
    output: &mut Vec<CodeSymbol>,
) {
    if let Some(symbol) = parse_document_symbol(entry, workspace_root, document_path) {
        output.push(symbol);
    }
    if let Some(children) = entry.get("children").and_then(Value::as_array) {
        for child in children {
            collect_document_symbols(child, workspace_root, document_path, output);
        }
    }
}

fn parse_document_symbol(
    entry: &Value,
    workspace_root: &Path,
    document_path: &Path,
) -> Option<CodeSymbol> {
    let name = entry.get("name")?.as_str()?.to_string();
    let uri = entry
        .get("uri")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| file_uri_from_path(document_path));
    let selection_range = entry
        .get("selectionRange")
        .or_else(|| entry.get("range"))
        .or_else(|| entry.pointer("/location/range"))?;
    let location = parse_uri_and_range(uri.as_str(), selection_range, workspace_root)?;
    Some(CodeSymbol {
        name,
        kind: parse_symbol_kind(entry.get("kind")),
        location,
        signature: entry
            .get("detail")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
    })
}

fn parse_locations_as_symbols(
    value: &Value,
    workspace_root: &Path,
    symbol_name: &str,
    kind: CodeSymbolKind,
) -> Vec<CodeSymbol> {
    collect_locations(value, workspace_root)
        .into_iter()
        .map(|location| CodeSymbol {
            name: symbol_name.to_string(),
            kind,
            location,
            signature: None,
        })
        .collect()
}

async fn parse_locations_as_references(
    value: &Value,
    workspace_root: &Path,
    symbol_name: &str,
) -> Vec<CodeReference> {
    let mut references = Vec::new();
    for location in collect_locations(value, workspace_root) {
        let absolute_path = workspace_root.join(&location.path);
        let line_text = fs::read_to_string(&absolute_path)
            .await
            .ok()
            .and_then(|source| {
                source
                    .lines()
                    .nth(location.line.saturating_sub(1))
                    .map(compact_line)
            })
            .unwrap_or_default();
        references.push(CodeReference {
            symbol: symbol_name.to_string(),
            location,
            line_text,
            is_definition: false,
        });
    }
    references
}

fn collect_locations(value: &Value, workspace_root: &Path) -> Vec<CodeLocation> {
    match value {
        Value::Array(entries) => entries
            .iter()
            .filter_map(|entry| parse_location_like(entry, workspace_root))
            .collect(),
        Value::Object(_) => parse_location_like(value, workspace_root)
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

fn parse_location_like(value: &Value, workspace_root: &Path) -> Option<CodeLocation> {
    if let Some(uri) = value.get("uri").and_then(Value::as_str) {
        let range = value.get("range")?;
        return parse_uri_and_range(uri, range, workspace_root);
    }
    if let Some(uri) = value.get("targetUri").and_then(Value::as_str) {
        let range = value
            .get("targetSelectionRange")
            .or_else(|| value.get("targetRange"))?;
        return parse_uri_and_range(uri, range, workspace_root);
    }
    None
}

fn parse_uri_and_range(uri: &str, range: &Value, workspace_root: &Path) -> Option<CodeLocation> {
    let path = file_uri_to_path(uri)?;
    let display_path = display_path(workspace_root, &path);
    let line = range.pointer("/start/line")?.as_u64()? as usize + 1;
    let column = range.pointer("/start/character")?.as_u64()? as usize + 1;
    Some(CodeLocation {
        path: display_path,
        line,
        column,
    })
}

fn parse_symbol_kind(value: Option<&Value>) -> CodeSymbolKind {
    match value.and_then(Value::as_u64).unwrap_or_default() {
        2 | 3 | 4 => CodeSymbolKind::Module,
        5 => CodeSymbolKind::Class,
        6 | 9 | 12 | 24 => CodeSymbolKind::Function,
        10 | 22 => CodeSymbolKind::Enum,
        11 => CodeSymbolKind::Interface,
        13 | 15 | 16 | 17 | 18 | 20 | 21 | 25 => CodeSymbolKind::Variable,
        14 => CodeSymbolKind::Constant,
        19 => CodeSymbolKind::Struct,
        23 => CodeSymbolKind::Struct,
        26 => CodeSymbolKind::TypeAlias,
        _ => CodeSymbolKind::Unknown,
    }
}

fn merge_symbols(
    semantic: Vec<CodeSymbol>,
    lexical: Vec<CodeSymbol>,
    limit: usize,
) -> Vec<CodeSymbol> {
    let mut seen = BTreeSet::new();
    let mut merged = Vec::new();
    for symbol in semantic.into_iter().chain(lexical) {
        let key = (
            symbol.name.clone(),
            symbol.location.path.clone(),
            symbol.location.line,
            symbol.location.column,
        );
        if seen.insert(key) {
            merged.push(symbol);
        }
        if merged.len() >= limit {
            break;
        }
    }
    merged
}

fn file_uri_from_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    let mut encoded = String::with_capacity(path.len() + 8);
    encoded.push_str("file://");
    for byte in path.as_bytes() {
        let ch = *byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '/' | '-' | '_' | '.' | '~') {
            encoded.push(ch);
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{byte:02X}"));
        }
    }
    encoded
}

fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let raw = uri.strip_prefix("file://")?;
    let mut decoded = Vec::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8(decoded).ok()?))
}

fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn zero_based_position(line: usize, column: usize) -> Value {
    json!({
        "line": line.saturating_sub(1),
        "character": column.saturating_sub(1),
    })
}

fn symbol_name_for_target(target: &CodeNavigationTarget) -> Option<String> {
    match target {
        CodeNavigationTarget::Symbol(symbol) => Some(symbol.trim().to_string()),
        CodeNavigationTarget::Position {
            path, line, column, ..
        } => identifier_at_position(path, *line, *column),
    }
}

fn identifier_at_position(path: &Path, line: usize, column: usize) -> Option<String> {
    let source = std::fs::read_to_string(path).ok()?;
    let line_text = source.lines().nth(line.saturating_sub(1))?;
    let cursor = column.saturating_sub(1).min(line_text.len());
    let bytes = line_text.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    let mut start = cursor.min(bytes.len().saturating_sub(1));
    while start > 0 && is_identifier_byte(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = cursor.min(bytes.len());
    while end < bytes.len() && is_identifier_byte(bytes[end]) {
        end += 1;
    }
    (start < end).then(|| line_text[start..end].to_string())
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

fn compact_line(line: &str) -> String {
    let compact = line.trim();
    let mut shortened = compact.chars().take(240).collect::<String>();
    if compact.chars().count() > 240 {
        shortened.push_str("...");
    }
    shortened
}

#[cfg(test)]
mod tests {
    use super::{
        CLANGD_SPEC, DOCKERFILE_SPEC, GO_SPEC, HTML_SPEC, JAVA_SPEC, JSON_SPEC, PHP_SPEC,
        PYTHON_SPEC, RUST_SPEC, SHELL_SPEC, SQL_SPEC, TOML_SPEC, TYPESCRIPT_SPEC, YAML_SPEC,
        build_cargo_install_args, build_npm_install_args, build_pip_install_args,
        file_uri_from_path, file_uri_to_path, identifier_at_position, language_id_for_path,
        managed_executable_path, merge_symbols, parse_location_like, parse_symbol_kind,
        server_spec_for_path,
    };
    use crate::code_intel::{CodeLocation, CodeSymbol, CodeSymbolKind};
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn spec_detection_covers_supported_languages() {
        assert_eq!(
            server_spec_for_path(Path::new("src/app.ts")).unwrap().id,
            TYPESCRIPT_SPEC.id
        );
        assert_eq!(
            server_spec_for_path(Path::new("templates/index.html"))
                .unwrap()
                .id,
            HTML_SPEC.id
        );
        assert_eq!(
            server_spec_for_path(Path::new("config/settings.json"))
                .unwrap()
                .id,
            JSON_SPEC.id
        );
        assert_eq!(
            server_spec_for_path(Path::new("src/main.py")).unwrap().id,
            PYTHON_SPEC.id
        );
        assert_eq!(
            server_spec_for_path(Path::new("src/main.go")).unwrap().id,
            GO_SPEC.id
        );
        assert_eq!(
            server_spec_for_path(Path::new("src/lib.rs")).unwrap().id,
            RUST_SPEC.id
        );
        assert_eq!(
            server_spec_for_path(Path::new("src/config.yaml"))
                .unwrap()
                .id,
            YAML_SPEC.id
        );
        assert_eq!(
            server_spec_for_path(Path::new("src/build.sh")).unwrap().id,
            SHELL_SPEC.id
        );
        assert_eq!(
            server_spec_for_path(Path::new(".bashrc")).unwrap().id,
            SHELL_SPEC.id
        );
        assert_eq!(
            server_spec_for_path(Path::new("Dockerfile")).unwrap().id,
            DOCKERFILE_SPEC.id
        );
        assert_eq!(
            server_spec_for_path(Path::new("php/index.php")).unwrap().id,
            PHP_SPEC.id
        );
        assert_eq!(
            server_spec_for_path(Path::new("Cargo.toml")).unwrap().id,
            TOML_SPEC.id
        );
        assert_eq!(
            server_spec_for_path(Path::new("queries/report.sql"))
                .unwrap()
                .id,
            SQL_SPEC.id
        );
        assert_eq!(
            server_spec_for_path(Path::new("src/main.cpp")).unwrap().id,
            CLANGD_SPEC.id
        );
        assert_eq!(
            server_spec_for_path(Path::new("src/Main.java")).unwrap().id,
            JAVA_SPEC.id
        );
    }

    #[test]
    fn language_ids_match_supported_extensions() {
        assert_eq!(
            language_id_for_path(Path::new("x.tsx")),
            Some("typescriptreact")
        );
        assert_eq!(
            language_id_for_path(Path::new("x.jsx")),
            Some("javascriptreact")
        );
        assert_eq!(
            language_id_for_path(Path::new("styles/app.scss")),
            Some("scss")
        );
        assert_eq!(
            language_id_for_path(Path::new("settings.code-workspace")),
            Some("jsonc")
        );
        assert_eq!(language_id_for_path(Path::new("x.yaml")), Some("yaml"));
        assert_eq!(language_id_for_path(Path::new("x.go")), Some("go"));
        assert_eq!(language_id_for_path(Path::new("go.mod")), Some("go.mod"));
        assert_eq!(
            language_id_for_path(Path::new("Dockerfile.dev")),
            Some("dockerfile")
        );
        assert_eq!(
            language_id_for_path(Path::new("src/main.m")),
            Some("objective-c")
        );
    }

    #[test]
    fn language_detection_is_separate_from_server_installation() {
        assert_eq!(
            server_spec_for_path(Path::new("frontend/app.ts"))
                .unwrap()
                .id,
            TYPESCRIPT_SPEC.id
        );
        assert_eq!(
            server_spec_for_path(Path::new("frontend/app.jsx"))
                .unwrap()
                .id,
            TYPESCRIPT_SPEC.id
        );
        assert_eq!(
            language_id_for_path(Path::new("frontend/app.ts")),
            Some("typescript")
        );
        assert_eq!(
            language_id_for_path(Path::new("frontend/app.jsx")),
            Some("javascriptreact")
        );
    }

    #[test]
    fn uri_round_trip_preserves_workspace_paths() {
        let path = Path::new("/tmp/hello world/src/lib.rs");
        let uri = file_uri_from_path(path);
        assert_eq!(file_uri_to_path(&uri).unwrap(), path);
    }

    #[test]
    fn identifier_lookup_reads_symbol_under_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("src.rs");
        std::fs::write(&path, "fn compute_total() { compute_total(); }\n").unwrap();
        assert_eq!(
            identifier_at_position(&path, 1, 5).unwrap(),
            "compute_total"
        );
        assert_eq!(
            identifier_at_position(&path, 1, 23).unwrap(),
            "compute_total"
        );
    }

    #[test]
    fn managed_binary_paths_match_installer_layouts() {
        let root = Path::new("/tmp/nanoclaw-lsp");
        assert_eq!(
            managed_executable_path(root, &TYPESCRIPT_SPEC),
            root.join("typescript/node_modules/.bin/typescript-language-server")
        );
        assert_eq!(
            managed_executable_path(root, &HTML_SPEC),
            root.join("html/node_modules/.bin/vscode-html-language-server")
        );
        assert_eq!(
            managed_executable_path(root, &PYTHON_SPEC),
            root.join("python/bin/pylsp")
        );
        assert_eq!(
            managed_executable_path(root, &GO_SPEC),
            root.join("go/bin/gopls")
        );
        assert_eq!(
            managed_executable_path(root, &TOML_SPEC),
            root.join("toml/bin/taplo")
        );
        assert_eq!(
            managed_executable_path(root, &SQL_SPEC),
            root.join("sql/bin/sqls")
        );
    }

    #[test]
    fn installer_args_include_prefix_install_root() {
        let root = Path::new("/tmp/server");
        assert_eq!(
            build_npm_install_args(root, &["typescript", "typescript-language-server"]),
            vec![
                "install",
                "--prefix",
                "/tmp/server",
                "--no-save",
                "typescript",
                "typescript-language-server",
            ]
        );
        assert_eq!(
            build_pip_install_args(root, &["python-lsp-server"]),
            vec![
                "-m",
                "pip",
                "install",
                "--upgrade",
                "--prefix",
                "/tmp/server",
                "python-lsp-server",
            ]
        );
        assert_eq!(
            build_cargo_install_args(root, "taplo-cli"),
            vec!["install", "--root", "/tmp/server", "--locked", "taplo-cli",]
        );
    }

    #[test]
    fn parser_supports_location_links() {
        let workspace_root = Path::new("/tmp/work");
        let entry = json!({
            "targetUri": "file:///tmp/work/src/lib.rs",
            "targetSelectionRange": {
                "start": { "line": 9, "character": 4 }
            }
        });
        let location = parse_location_like(&entry, workspace_root).unwrap();
        assert_eq!(location.path, "src/lib.rs");
        assert_eq!(location.line, 10);
        assert_eq!(location.column, 5);
    }

    #[test]
    fn symbol_kind_mapping_handles_common_lsp_kinds() {
        assert_eq!(
            parse_symbol_kind(Some(&json!(12))),
            CodeSymbolKind::Function
        );
        assert_eq!(parse_symbol_kind(Some(&json!(23))), CodeSymbolKind::Struct);
        assert_eq!(
            parse_symbol_kind(Some(&json!(14))),
            CodeSymbolKind::Constant
        );
    }

    #[test]
    fn merge_dedupes_semantic_and_lexical_results() {
        let semantic = vec![CodeSymbol {
            name: "Engine".into(),
            kind: CodeSymbolKind::Struct,
            location: CodeLocation {
                path: "src/lib.rs".into(),
                line: 4,
                column: 12,
            },
            signature: None,
        }];
        let lexical = vec![
            semantic[0].clone(),
            CodeSymbol {
                name: "Runner".into(),
                kind: CodeSymbolKind::Struct,
                location: CodeLocation {
                    path: "src/app.rs".into(),
                    line: 2,
                    column: 8,
                },
                signature: None,
            },
        ];
        let merged = merge_symbols(semantic, lexical, 8);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].name, "Engine");
        assert_eq!(merged[1].name, "Runner");
    }
}
