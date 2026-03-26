mod protocol;
mod support;

use crate::code_intel::{
    CodeIntelBackend, CodeNavigationTarget, CodeReference, CodeSymbol,
    WorkspaceTextCodeIntelBackend,
};
use crate::file_activity::FileActivityObserver;
use crate::process::{
    ExecRequest, ExecutionOrigin, ProcessExecutor, ProcessStdio, RuntimeScope, SandboxPolicy,
};
use crate::{Result, ToolError, ToolExecutionContext, stable_text_hash};
use async_trait::async_trait;
use notify::{
    Config as NotifyConfig, Event as NotifyEvent, RecommendedWatcher, RecursiveMode, Watcher,
};
use protocol::{
    DiagnosticEntry, configuration_response, file_uri_from_path, file_uri_to_path,
    identifier_at_position, parse_diagnostic_entry, parse_document_symbols,
    parse_locations_as_references, parse_locations_as_symbols, parse_workspace_symbols,
    read_lsp_message, zero_based_position,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::{
    InstallStrategy, LanguageServerSpec, ServerFamily, WatchRegistration, WorkspaceWatchEvent,
    build_cargo_install_args, build_npm_install_args, build_pip_install_args,
    collect_high_priority_files, collect_preload_candidates, collect_workspace_events,
    extract_watch_registrations, is_high_priority_file, language_id_for_path,
    managed_executable_path, preload_limit_for_server, server_family, server_spec_for_path,
    should_exclude_workspace_path, should_preload_path,
};
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot};
use tokio::time::{Instant, sleep, timeout};
use tracing::{debug, info, warn};

const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(20);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);
const READY_TIMEOUT: Duration = Duration::from_secs(30);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(500);
const WATCH_DEBOUNCE: Duration = Duration::from_millis(300);

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

    fn did_save(&self, path: PathBuf) {
        self.runtime.spawn_sync(path, FileSyncEvent::Save);
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
    watcher_started: AtomicBool,
    watcher: Mutex<Option<RecommendedWatcher>>,
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
            watcher_started: AtomicBool::new(false),
            watcher: Mutex::new(None),
        }
    }

    fn spawn_sync(self: &Arc<Self>, path: PathBuf, event: FileSyncEvent) {
        if !self.options.enabled {
            return;
        }
        if let Err(error) = self.ensure_watcher_started() {
            debug!("managed LSP watcher disabled: {error}");
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

    async fn handle_file_event(
        self: &Arc<Self>,
        path: PathBuf,
        event: FileSyncEvent,
    ) -> Result<()> {
        match event {
            FileSyncEvent::Open | FileSyncEvent::Change => {
                self.ensure_document_synced(&path).await?;
            }
            FileSyncEvent::Save => {
                self.ensure_document_synced(&path).await?;
                self.notify_document_saved(&path).await?;
            }
            FileSyncEvent::Remove => {
                self.close_document(&path).await?;
            }
        }
        Ok(())
    }

    fn ensure_watcher_started(self: &Arc<Self>) -> Result<()> {
        if !self.options.enabled {
            return Ok(());
        }
        if self
            .watcher_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(());
        }

        let (tx, mut rx) = mpsc::unbounded_channel::<notify::Result<NotifyEvent>>();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = tx.send(event);
        })
        .map_err(|error| {
            ToolError::invalid_state(format!(
                "failed to create managed LSP workspace watcher: {error}"
            ))
        })?;
        watcher
            .configure(NotifyConfig::default())
            .map_err(|error| {
                ToolError::invalid_state(format!(
                    "failed to configure managed LSP workspace watcher: {error}"
                ))
            })?;
        watcher
            .watch(&self.workspace_root, RecursiveMode::Recursive)
            .map_err(|error| {
                ToolError::invalid_state(format!(
                    "failed to watch workspace {} for managed LSP updates: {error}",
                    self.workspace_root.display()
                ))
            })?;
        *self.watcher.lock().expect("LSP watcher lock") = Some(watcher);

        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            let mut pending = BTreeMap::<PathBuf, WorkspaceWatchEvent>::new();
            let timer = sleep(Duration::MAX);
            tokio::pin!(timer);

            loop {
                tokio::select! {
                    maybe_event = rx.recv() => {
                        let Some(event) = maybe_event else {
                            break;
                        };
                        match event {
                            Ok(event) => {
                                for (path, kind) in collect_workspace_events(&event) {
                                    if should_exclude_workspace_path(&path) {
                                        continue;
                                    }
                                    pending
                                        .entry(path)
                                        .and_modify(|existing| *existing = existing.merge(kind))
                                        .or_insert(kind);
                                }
                                if !pending.is_empty() {
                                    timer.as_mut().reset(Instant::now() + WATCH_DEBOUNCE);
                                }
                            }
                            Err(error) => {
                                debug!("managed LSP watcher event dropped: {error}");
                            }
                        }
                    }
                    _ = &mut timer, if !pending.is_empty() => {
                        let events = std::mem::take(&mut pending);
                        for (path, kind) in events {
                            if let Err(error) = runtime.handle_workspace_watch_event(path.clone(), kind).await {
                                debug!(
                                    "managed LSP workspace event ignored for {}: {error}",
                                    path.display()
                                );
                            }
                        }
                        timer.as_mut().reset(Instant::now() + Duration::MAX);
                    }
                }
            }
        });

        Ok(())
    }

    async fn notify_document_saved(self: &Arc<Self>, path: &Path) -> Result<()> {
        let Some(session) = self.ensure_session_for_path(path).await? else {
            return Ok(());
        };
        session
            .notify(
                "textDocument/didSave",
                json!({
                    "textDocument": {
                        "uri": file_uri_from_path(path)
                    }
                }),
            )
            .await
    }

    async fn workspace_symbols(
        self: &Arc<Self>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<CodeSymbol>> {
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

    async fn document_symbols(
        self: &Arc<Self>,
        path: &Path,
        limit: usize,
    ) -> Result<Vec<CodeSymbol>> {
        let Some(session) = self.ensure_session_for_path(path).await? else {
            return Ok(Vec::new());
        };
        self.ensure_document_synced(path).await?;
        let response = session
            .request(
                "textDocument/documentSymbol",
                json!({ "textDocument": { "uri": file_uri_from_path(path) } }),
            )
            .await?;
        let mut symbols = parse_document_symbols(&response, &self.workspace_root, path);
        symbols.truncate(limit);
        Ok(symbols)
    }

    async fn definitions(
        self: &Arc<Self>,
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
                    crate::code_intel::CodeSymbolKind::Unknown,
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
        self: &Arc<Self>,
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
                    .filter(|session| session.is_alive() && session.is_ready())
            })
            .collect()
    }

    async fn ensure_session_for_path(
        self: &Arc<Self>,
        path: &Path,
    ) -> Result<Option<Arc<LspSession>>> {
        if !self.options.enabled {
            return Ok(None);
        }
        if let Err(error) = self.ensure_watcher_started() {
            debug!("managed LSP watcher unavailable: {error}");
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
        // when file hooks and semantic queries race the same server.
        let mut state = slot.state.lock().await;
        if let Some(session) = state.as_ref() {
            if session.is_alive() {
                return Ok(Some(session.clone()));
            }
            *state = None;
        }

        let command = match self.resolve_command(spec).await {
            Ok(command) => command,
            Err(error) => {
                self.log_unavailable_once(spec, &error.to_string());
                return Ok(None);
            }
        };
        let session = LspSession::start(
            spec,
            self.workspace_root.clone(),
            command,
            self.process_executor.clone(),
            self.server_policy.clone(),
        )
        .await?;
        self.spawn_session_warmup(session.clone());
        *state = Some(session.clone());
        Ok(Some(session))
    }

    async fn ensure_document_synced(self: &Arc<Self>, path: &Path) -> Result<()> {
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

    async fn close_document(self: &Arc<Self>, path: &Path) -> Result<()> {
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

    fn spawn_session_warmup(self: &Arc<Self>, session: Arc<LspSession>) {
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(error) = runtime.warmup_session(session.clone()).await {
                debug!(
                    "managed LSP warmup skipped for {}: {error}",
                    session.spec.id
                );
            }
        });
    }

    async fn warmup_session(self: &Arc<Self>, session: Arc<LspSession>) -> Result<()> {
        if !session.start_warmup() {
            return Ok(());
        }

        for path in collect_high_priority_files(&self.workspace_root, session.spec) {
            session.open_document_path(path.as_path()).await?;
        }

        let preload_limit = preload_limit_for_server(session.spec);
        if preload_limit > 0 {
            for path in
                collect_preload_candidates(&self.workspace_root, session.spec, preload_limit)
            {
                session.open_document_path(path.as_path()).await?;
            }
        }

        session.wait_for_ready().await
    }

    async fn handle_workspace_watch_event(
        self: &Arc<Self>,
        path: PathBuf,
        kind: WorkspaceWatchEvent,
    ) -> Result<()> {
        if kind != WorkspaceWatchEvent::Deleted && (!path.exists() || !path.is_file()) {
            return Ok(());
        }

        for session in self.ready_sessions() {
            if !session.should_track_workspace_path(path.as_path(), kind) {
                continue;
            }

            match kind {
                WorkspaceWatchEvent::Changed if session.is_document_open(path.as_path()) => {
                    if let Err(error) = session.sync_document_from_disk(path.as_path()).await {
                        debug!(
                            "managed LSP didChange for {} failed: {error}",
                            path.display()
                        );
                    }
                }
                WorkspaceWatchEvent::Deleted => {
                    let _ = session.close_document(path.as_path()).await;
                }
                WorkspaceWatchEvent::Created => {
                    if should_preload_path(path.as_path(), session.spec)
                        || is_high_priority_file(path.as_path(), session.spec)
                    {
                        let _ = session.open_document_path(path.as_path()).await;
                    }
                }
                WorkspaceWatchEvent::Changed => {}
            }

            let _ = session.notify_watched_file(path.as_path(), kind).await;
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
                            ToolError::invalid_state("go is required for managed Go LSP installs")
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
                            "python3 or python is required for managed Python LSP installs",
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
    Save,
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
    diagnostics: Mutex<BTreeMap<PathBuf, Vec<DiagnosticEntry>>>,
    watch_registrations: Mutex<Vec<WatchRegistration>>,
    alive: AtomicBool,
    ready: AtomicBool,
    warmup_started: AtomicBool,
}

impl LspSession {
    async fn start(
        spec: &'static LanguageServerSpec,
        workspace_root: PathBuf,
        command: ResolvedCommand,
        process_executor: Arc<dyn ProcessExecutor>,
        sandbox_policy: SandboxPolicy,
    ) -> Result<Arc<Self>> {
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

        let session = Arc::new(Self {
            spec,
            workspace_root: workspace_root.clone(),
            child: Mutex::new(child),
            stdin: AsyncMutex::new(stdin),
            next_id: AtomicI64::new(1),
            pending: Arc::new(Mutex::new(BTreeMap::new())),
            documents: Mutex::new(BTreeMap::new()),
            diagnostics: Mutex::new(BTreeMap::new()),
            watch_registrations: Mutex::new(Vec::new()),
            alive: AtomicBool::new(true),
            ready: AtomicBool::new(false),
            warmup_started: AtomicBool::new(false),
        });
        session.spawn_reader(stdout);
        session.spawn_stderr(stderr);
        if let Err(error) = session.initialize().await {
            session.mark_dead("initialize failed");
            return Err(error);
        }
        Ok(session)
    }

    fn spawn_reader(self: &Arc<Self>, stdout: tokio::process::ChildStdout) {
        let session = Arc::clone(self);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                let message = read_lsp_message(&mut reader).await;
                let Some(message) = message.transpose() else {
                    session.mark_dead("reader closed");
                    break;
                };
                let Ok(message) = message else {
                    warn!(
                        "managed LSP reader for {} stopped with error",
                        session.spec.id
                    );
                    session.mark_dead("reader failed");
                    break;
                };

                if message.get("id").is_some() && message.get("method").is_some() {
                    session.handle_server_request(message).await;
                    continue;
                }
                if message.get("method").is_some() {
                    session.handle_server_notification(message);
                    continue;
                }
                if let Some(id) = message.get("id").and_then(Value::as_i64) {
                    let sender = session
                        .pending
                        .lock()
                        .expect("LSP pending map lock")
                        .remove(&id);
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
            session.fail_pending_requests("managed LSP transport stopped");
        });
    }

    fn spawn_stderr(self: &Arc<Self>, stderr: tokio::process::ChildStderr) {
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

    async fn initialize(self: &Arc<Self>) -> Result<()> {
        let root_uri = file_uri_from_path(&self.workspace_root);
        let root_name = self
            .workspace_root
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("workspace");
        timeout(
            INITIALIZE_TIMEOUT,
            self.request(
                "initialize",
                json!({
                    "processId": std::process::id(),
                    "rootUri": root_uri,
                    "rootPath": self.workspace_root.display().to_string(),
                    "workspaceFolders": [{ "uri": root_uri, "name": root_name }],
                    "capabilities": {
                        "workspace": {
                            "configuration": true,
                            "didChangeConfiguration": { "dynamicRegistration": true },
                            "didChangeWatchedFiles": {
                                "dynamicRegistration": true,
                                "relativePatternSupport": true
                            },
                            "symbol": {}
                        },
                        "textDocument": {
                            "definition": { "linkSupport": true },
                            "references": {},
                            "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                            "synchronization": { "didSave": true },
                            "publishDiagnostics": { "versionSupport": true }
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
        self.notify("initialized", json!({})).await?;
        info!("managed LSP server {} initialized", self.spec.id);
        Ok(())
    }

    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    fn start_warmup(&self) -> bool {
        self.warmup_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    fn is_document_open(&self, path: &Path) -> bool {
        self.documents
            .lock()
            .expect("LSP document map lock")
            .contains_key(path)
    }

    fn clear_diagnostics(&self, path: &Path) {
        self.diagnostics
            .lock()
            .expect("LSP diagnostics map lock")
            .remove(path);
    }

    fn mark_dead(&self, reason: &str) {
        if self.alive.swap(false, Ordering::SeqCst) {
            self.ready.store(false, Ordering::SeqCst);
            debug!("managed LSP {} marked dead: {reason}", self.spec.id);
        }
    }

    fn fail_pending_requests(&self, reason: &str) {
        let pending = std::mem::take(&mut *self.pending.lock().expect("LSP pending map lock"));
        for (_, sender) in pending {
            let _ = sender.send(Err(reason.to_string()));
        }
    }

    async fn wait_for_ready(self: &Arc<Self>) -> Result<()> {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if self.ping_server().await.is_ok() {
                self.ready.store(true, Ordering::SeqCst);
                return Ok(());
            }
            if Instant::now() >= deadline {
                self.mark_dead("readiness timeout");
                return Err(ToolError::invalid_state(format!(
                    "timed out waiting for managed LSP {} readiness",
                    self.spec.id
                )));
            }
            sleep(READY_POLL_INTERVAL).await;
        }
    }

    async fn ping_server(&self) -> Result<()> {
        if matches!(server_family(self.spec), ServerFamily::TypeScript) {
            let typescript_path = {
                let documents = self.documents.lock().expect("LSP document map lock");
                documents
                    .keys()
                    .find(|path| {
                        matches!(
                            language_id_for_path(path.as_path()),
                            Some(
                                "typescript" | "typescriptreact" | "javascript" | "javascriptreact"
                            )
                        )
                    })
                    .cloned()
            };
            if let Some(path) = typescript_path {
                let _ = self
                    .request(
                        "textDocument/documentSymbol",
                        json!({ "textDocument": { "uri": file_uri_from_path(path.as_path()) } }),
                    )
                    .await?;
                return Ok(());
            }
        }

        let _ = self
            .request("workspace/symbol", json!({ "query": "" }))
            .await?;
        Ok(())
    }

    async fn handle_server_request(self: &Arc<Self>, message: Value) {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let response = match method {
            "client/registerCapability" => {
                self.update_watch_registrations(&params);
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": Value::Null,
                })
            }
            "workspace/configuration" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": configuration_response(&params),
            }),
            "workspace/applyEdit" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "applied": false },
            }),
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32601,
                    "message": format!("unsupported client request: {method}"),
                }
            }),
        };
        if let Err(error) = self.write_message(&response).await {
            debug!(
                "managed LSP {} failed to answer server request {}: {error}",
                self.spec.id, method
            );
        }
    }

    fn handle_server_notification(&self, message: Value) {
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        match method {
            "textDocument/publishDiagnostics" => self.apply_diagnostics(&params),
            "window/showMessage" => {
                if let Some(payload) = params.get("message").and_then(Value::as_str) {
                    debug!("LSP server message [{}] {payload}", self.spec.id);
                }
            }
            _ => {}
        }
    }

    fn apply_diagnostics(&self, params: &Value) {
        let Some(uri) = params.get("uri").and_then(Value::as_str) else {
            return;
        };
        let Some(path) = file_uri_to_path(uri) else {
            return;
        };
        let diagnostics = params
            .get("diagnostics")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| parse_diagnostic_entry(entry, path.as_path(), &self.workspace_root))
            .collect::<Vec<_>>();
        self.diagnostics
            .lock()
            .expect("LSP diagnostics map lock")
            .insert(path, diagnostics);
    }

    fn update_watch_registrations(&self, params: &Value) {
        *self
            .watch_registrations
            .lock()
            .expect("LSP watch registrations lock") = extract_watch_registrations(params);
    }

    fn should_track_workspace_path(&self, path: &Path, kind: WorkspaceWatchEvent) -> bool {
        let registrations = self
            .watch_registrations
            .lock()
            .expect("LSP watch registrations lock");
        if registrations.is_empty() {
            return true;
        }
        registrations
            .iter()
            .any(|registration| registration.matches(&self.workspace_root, path, kind))
    }

    async fn open_document_path(&self, path: &Path) -> Result<()> {
        self.sync_document_from_disk(path).await
    }

    async fn sync_document_from_disk(&self, path: &Path) -> Result<()> {
        if !path.exists() || !path.is_file() {
            return Ok(());
        }
        let Some(language_id) = language_id_for_path(path) else {
            return Ok(());
        };
        let content = fs::read_to_string(path).await.map_err(|error| {
            ToolError::invalid_state(format!(
                "failed to read {} for LSP sync: {error}",
                path.display()
            ))
        })?;
        self.sync_document(path, language_id, content).await
    }

    async fn notify_watched_file(&self, path: &Path, kind: WorkspaceWatchEvent) -> Result<()> {
        self.notify(
            "workspace/didChangeWatchedFiles",
            json!({
                "changes": [{
                    "uri": file_uri_from_path(path),
                    "type": kind.lsp_kind(),
                }]
            }),
        )
        .await
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        if !self.is_alive() {
            return Err(ToolError::invalid_state(format!(
                "managed LSP {} is not alive",
                self.spec.id
            )));
        }
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
                self.mark_dead("request timeout");
                ToolError::invalid_state(format!(
                    "timed out waiting for LSP {} response to {}",
                    self.spec.id, method
                ))
            })?
            .map_err(|_| {
                self.mark_dead("response channel dropped");
                ToolError::invalid_state(format!(
                    "managed LSP {} dropped the response channel for {}",
                    self.spec.id, method
                ))
            })?
            .map_err(|error| {
                self.mark_dead("server returned transport error");
                ToolError::invalid_state(error)
            })
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
        if !self.is_alive() {
            return Err(ToolError::invalid_state(format!(
                "managed LSP {} is not alive",
                self.spec.id
            )));
        }
        let body = serde_json::to_vec(message).map_err(|error| {
            ToolError::invalid_state(format!("failed to serialize LSP payload: {error}"))
        })?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(header.as_bytes()).await.map_err(|error| {
            self.mark_dead("stdin header write failed");
            ToolError::invalid_state(format!(
                "failed to write LSP header to {}: {error}",
                self.spec.id
            ))
        })?;
        stdin.write_all(&body).await.map_err(|error| {
            self.mark_dead("stdin body write failed");
            ToolError::invalid_state(format!(
                "failed to write LSP body to {}: {error}",
                self.spec.id
            ))
        })?;
        stdin.flush().await.map_err(|error| {
            self.mark_dead("stdin flush failed");
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
        self.clear_diagnostics(path);
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
        // Drop can run while the async runtime is tearing down, so the full shutdown/exit
        // handshake is not reliable here. Runtime-driven close paths should emit normal LSP
        // notifications earlier; drop only prevents orphaned helper processes.
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

fn find_executable(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var).find_map(|dir| {
        let candidate = dir.join(name);
        candidate.is_file().then_some(candidate)
    })
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

fn symbol_name_for_target(target: &CodeNavigationTarget) -> Option<String> {
    match target {
        CodeNavigationTarget::Symbol(symbol) => Some(symbol.trim().to_string()),
        CodeNavigationTarget::Position {
            path, line, column, ..
        } => identifier_at_position(path, *line, *column),
    }
}

#[cfg(test)]
mod tests {
    use super::merge_symbols;
    use crate::code_intel::{CodeLocation, CodeSymbol, CodeSymbolKind};

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
