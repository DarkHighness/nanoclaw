use super::protocol::file_uri_to_path;
use globset::{Glob, GlobMatcher};
use ignore::WalkBuilder;
use notify::Event;
use notify::event::{CreateKind, EventKind, ModifyKind, RemoveKind};
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

const MAX_TRACKED_FILE_BYTES: u64 = 5 * 1024 * 1024;
const MAX_PRELOAD_FILE_BYTES: u64 = 1024 * 1024;

/// Runtime-visible install strategies that address a single package manager.
#[derive(Clone, Copy)]
pub(crate) enum InstallStrategy {
    Npm { packages: &'static [&'static str] },
    Go { module: &'static str },
    Pip { packages: &'static [&'static str] },
    Cargo { package: &'static str },
}

/// Static descriptor that couples a language server binary with its install metadata.
#[derive(Clone, Copy)]
pub(crate) struct LanguageServerSpec {
    pub(crate) id: &'static str,
    pub(crate) install_id: &'static str,
    pub(crate) command: &'static str,
    pub(crate) args: &'static [&'static str],
    pub(crate) install: Option<InstallStrategy>,
}

/// Match table that translates a file signature into a language ID plus the server
/// that should own the resulting LSP session.
#[derive(Clone, Copy)]
pub(crate) struct LanguageSupport {
    pub(crate) language_id: &'static str,
    pub(crate) server: &'static LanguageServerSpec,
    pub(crate) extensions: &'static [&'static str],
    pub(crate) file_names: &'static [&'static str],
    pub(crate) file_name_prefixes: &'static [&'static str],
}

impl LanguageSupport {
    fn matches(&self, signature: &PathSignature) -> bool {
        signature
            .extension
            .as_deref()
            .is_some_and(|ext| self.extensions.iter().any(|candidate| *candidate == ext))
            || signature.file_name.as_deref().is_some_and(|name| {
                self.file_names.iter().any(|candidate| *candidate == name)
                    || self
                        .file_name_prefixes
                        .iter()
                        .any(|candidate| name.starts_with(candidate))
            })
    }
}

#[derive(Default)]
pub(crate) struct PathSignature {
    pub(crate) extension: Option<String>,
    pub(crate) file_name: Option<String>,
}

impl PathSignature {
    pub(crate) fn from_path(path: &Path) -> Self {
        Self {
            extension: lowercase_extension(path),
            file_name: lowercase_file_name(path),
        }
    }
}

pub(crate) const TYPESCRIPT_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "typescript",
    install_id: "typescript",
    command: "typescript-language-server",
    args: &["--stdio"],
    install: Some(InstallStrategy::Npm {
        packages: &["typescript", "typescript-language-server"],
    }),
};

pub(crate) const HTML_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "html",
    install_id: "html",
    command: "vscode-html-language-server",
    args: &["--stdio"],
    install: Some(InstallStrategy::Npm {
        packages: &["vscode-langservers-extracted"],
    }),
};

pub(crate) const CSS_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "css",
    install_id: "css",
    command: "vscode-css-language-server",
    args: &["--stdio"],
    install: Some(InstallStrategy::Npm {
        packages: &["vscode-langservers-extracted"],
    }),
};

pub(crate) const JSON_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "json",
    install_id: "json",
    command: "vscode-json-language-server",
    args: &["--stdio"],
    install: Some(InstallStrategy::Npm {
        packages: &["vscode-langservers-extracted"],
    }),
};

pub(crate) const PYTHON_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "python",
    install_id: "python",
    command: "pylsp",
    args: &[],
    install: Some(InstallStrategy::Pip {
        packages: &["python-lsp-server"],
    }),
};

pub(crate) const GO_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "go",
    install_id: "go",
    command: "gopls",
    args: &[],
    install: Some(InstallStrategy::Go {
        module: "golang.org/x/tools/gopls@latest",
    }),
};

pub(crate) const YAML_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "yaml",
    install_id: "yaml",
    command: "yaml-language-server",
    args: &["--stdio"],
    install: Some(InstallStrategy::Npm {
        packages: &["yaml-language-server"],
    }),
};

pub(crate) const SHELL_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "shell",
    install_id: "shell",
    command: "bash-language-server",
    args: &["start"],
    install: Some(InstallStrategy::Npm {
        packages: &["bash-language-server"],
    }),
};

pub(crate) const DOCKERFILE_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "dockerfile",
    install_id: "dockerfile",
    command: "docker-langserver",
    args: &["--stdio"],
    install: Some(InstallStrategy::Npm {
        packages: &["dockerfile-language-server-nodejs"],
    }),
};

pub(crate) const PHP_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "php",
    install_id: "php",
    command: "intelephense",
    args: &["--stdio"],
    install: Some(InstallStrategy::Npm {
        packages: &["intelephense"],
    }),
};

pub(crate) const TOML_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "toml",
    install_id: "toml",
    command: "taplo",
    args: &["lsp", "stdio"],
    install: Some(InstallStrategy::Cargo {
        package: "taplo-cli",
    }),
};

pub(crate) const SQL_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "sql",
    install_id: "sql",
    command: "sqls",
    args: &[],
    install: Some(InstallStrategy::Go {
        module: "github.com/sqls-server/sqls@latest",
    }),
};

pub(crate) const RUST_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "rust",
    install_id: "rust",
    command: "rust-analyzer",
    args: &[],
    install: None,
};

pub(crate) const JAVA_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "java",
    install_id: "java",
    command: "jdtls",
    args: &[],
    install: None,
};

pub(crate) const CLANGD_SPEC: LanguageServerSpec = LanguageServerSpec {
    id: "clangd",
    install_id: "clangd",
    command: "clangd",
    args: &[],
    install: None,
};

pub(crate) const SUPPORTED_LANGUAGES: &[LanguageSupport] = &[
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

pub(crate) fn language_support_for_path(path: &Path) -> Option<&'static LanguageSupport> {
    let signature = PathSignature::from_path(path);
    SUPPORTED_LANGUAGES
        .iter()
        .find(|support| support.matches(&signature))
}

pub(crate) fn server_spec_for_path(path: &Path) -> Option<&'static LanguageServerSpec> {
    language_support_for_path(path).map(|support| support.server)
}

pub(crate) fn language_id_for_path(path: &Path) -> Option<&'static str> {
    language_support_for_path(path).map(|support| support.language_id)
}

pub(crate) fn managed_executable_path(install_root: &Path, spec: &LanguageServerSpec) -> PathBuf {
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

pub(crate) fn build_npm_install_args(server_root: &Path, packages: &[&str]) -> Vec<String> {
    let mut args = vec![
        "install".to_string(),
        "--prefix".to_string(),
        server_root.display().to_string(),
        "--no-save".to_string(),
    ];
    args.extend(packages.iter().map(|value| (*value).to_string()));
    args
}

pub(crate) fn build_pip_install_args(server_root: &Path, packages: &[&str]) -> Vec<String> {
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

pub(crate) fn build_cargo_install_args(server_root: &Path, package: &str) -> Vec<String> {
    vec![
        "install".to_string(),
        "--root".to_string(),
        server_root.display().to_string(),
        "--locked".to_string(),
        package.to_string(),
    ]
}

pub(crate) fn lowercase_extension(path: &Path) -> Option<String> {
    path.extension()?.to_str().map(str::to_ascii_lowercase)
}

pub(crate) fn lowercase_file_name(path: &Path) -> Option<String> {
    path.file_name()?.to_str().map(str::to_ascii_lowercase)
}

/// Server families gate warmup heuristics. This keeps recognition separate from
/// installation while still letting the runtime apply a few server-specific
/// behaviors that opencode also relies on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ServerFamily {
    TypeScript,
    Go,
    Rust,
    Python,
    Clangd,
    Java,
    Generic,
}

pub(crate) fn server_family(spec: &LanguageServerSpec) -> ServerFamily {
    match spec.id {
        "typescript" => ServerFamily::TypeScript,
        "go" => ServerFamily::Go,
        "rust" => ServerFamily::Rust,
        "python" => ServerFamily::Python,
        "clangd" => ServerFamily::Clangd,
        "java" => ServerFamily::Java,
        _ => ServerFamily::Generic,
    }
}

pub(crate) fn preload_limit_for_server(spec: &LanguageServerSpec) -> usize {
    match server_family(spec) {
        ServerFamily::TypeScript => 100,
        ServerFamily::Java => 200,
        _ => 0,
    }
}

pub(crate) fn is_high_priority_file(path: &Path, spec: &LanguageServerSpec) -> bool {
    let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    match spec.id {
        "typescript" => matches!(
            file_name,
            "tsconfig.json"
                | "package.json"
                | "jsconfig.json"
                | "index.ts"
                | "index.js"
                | "main.ts"
                | "main.js"
        ),
        "go" => matches!(file_name, "go.mod" | "go.sum" | "main.go"),
        "rust" => matches!(
            file_name,
            "Cargo.toml" | "Cargo.lock" | "lib.rs" | "main.rs"
        ),
        "python" => matches!(
            file_name,
            "pyproject.toml" | "setup.py" | "requirements.txt" | "__init__.py" | "__main__.py"
        ),
        "clangd" => matches!(
            file_name,
            "CMakeLists.txt" | "Makefile" | "compile_commands.json"
        ),
        "java" => {
            matches!(file_name, "pom.xml" | "build.gradle")
                || lowercase_extension(path).as_deref() == Some("java")
        }
        _ => false,
    }
}

pub(crate) fn should_preload_path(path: &Path, spec: &LanguageServerSpec) -> bool {
    preload_limit_for_server(spec) > 0
        && path.is_file()
        && !should_exclude_workspace_path(path)
        && fits_size_limit(path, MAX_PRELOAD_FILE_BYTES)
        && server_spec_for_path(path)
            .map(|candidate| candidate.id == spec.id)
            .unwrap_or(false)
}

pub(crate) fn collect_high_priority_files(
    workspace_root: &Path,
    spec: &'static LanguageServerSpec,
) -> Vec<PathBuf> {
    let limit = match server_family(spec) {
        ServerFamily::Java => 200,
        _ => 24,
    };
    collect_workspace_files(workspace_root, limit, |path| {
        is_high_priority_file(path, spec) && fits_size_limit(path, MAX_PRELOAD_FILE_BYTES)
    })
}

pub(crate) fn collect_preload_candidates(
    workspace_root: &Path,
    spec: &'static LanguageServerSpec,
    limit: usize,
) -> Vec<PathBuf> {
    collect_workspace_files(workspace_root, limit, |path| {
        should_preload_path(path, spec) && !is_high_priority_file(path, spec)
    })
}

fn collect_workspace_files(
    workspace_root: &Path,
    limit: usize,
    predicate: impl Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    let mut builder = WalkBuilder::new(workspace_root);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true);
    // Preload selection is bounded. Sort traversal so the chosen prefix stays
    // deterministic instead of depending on filesystem directory iteration.
    builder.sort_by_file_path(|left, right| left.cmp(right));
    builder.filter_entry(|entry| {
        if entry.depth() == 0 {
            return true;
        }
        if entry.file_type().is_some_and(|kind| kind.is_dir()) {
            return !should_exclude_workspace_path(entry.path());
        }
        true
    });

    let mut files = Vec::new();
    for result in builder.build() {
        let Ok(entry) = result else {
            continue;
        };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.into_path();
        if predicate(path.as_path()) {
            files.push(path);
        }
        if files.len() >= limit {
            break;
        }
    }
    files
}

fn fits_size_limit(path: &Path, limit: u64) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.len() <= limit)
        .unwrap_or(false)
}

pub(crate) struct WatchRegistration {
    matcher: GlobMatcher,
    base_path: Option<PathBuf>,
    watch_kind: u64,
}

impl WatchRegistration {
    fn from_watcher(watcher: &Value) -> Option<Self> {
        let watch_kind = watcher.get("kind").and_then(Value::as_u64).unwrap_or(7);
        let glob = watcher.get("globPattern")?;
        let (pattern, base_path) = parse_glob_pattern(glob)?;
        let matcher = Glob::new(pattern.as_str()).ok()?.compile_matcher();
        Some(Self {
            matcher,
            base_path,
            watch_kind,
        })
    }

    pub(crate) fn matches(
        &self,
        workspace_root: &Path,
        path: &Path,
        kind: WorkspaceWatchEvent,
    ) -> bool {
        if self.watch_kind & watch_kind_mask(kind) == 0 {
            return false;
        }

        let mut candidates = Vec::new();
        candidates.push(normalize_path_text(path));
        if let Some(relative) = path
            .strip_prefix(workspace_root)
            .ok()
            .map(normalize_path_text)
        {
            candidates.push(relative);
        }
        if let Some(base_path) = &self.base_path {
            if let Some(relative) = path.strip_prefix(base_path).ok().map(normalize_path_text) {
                candidates.push(relative);
            }
        }
        if let Some(file_name) = path.file_name().and_then(|value| value.to_str()) {
            candidates.push(file_name.to_string());
        }

        candidates
            .into_iter()
            .any(|candidate| self.matcher.is_match(candidate.as_str()))
    }
}

fn parse_glob_pattern(value: &Value) -> Option<(String, Option<PathBuf>)> {
    match value {
        Value::String(pattern) => Some((pattern.clone(), None)),
        Value::Object(map) => {
            let pattern = map.get("pattern")?.as_str()?.to_string();
            let base_path = map
                .get("baseUri")
                .and_then(parse_base_uri_value)
                .or_else(|| {
                    map.get("base_path")
                        .and_then(Value::as_str)
                        .map(PathBuf::from)
                });
            Some((pattern, base_path))
        }
        _ => None,
    }
}

fn parse_base_uri_value(value: &Value) -> Option<PathBuf> {
    value.as_str().and_then(file_uri_to_path).or_else(|| {
        value
            .get("uri")
            .and_then(Value::as_str)
            .and_then(file_uri_to_path)
    })
}

fn watch_kind_mask(kind: WorkspaceWatchEvent) -> u64 {
    match kind {
        WorkspaceWatchEvent::Created => 1,
        WorkspaceWatchEvent::Changed => 2,
        WorkspaceWatchEvent::Deleted => 4,
    }
}

pub(crate) fn extract_watch_registrations(params: &Value) -> Vec<WatchRegistration> {
    params
        .get("registrations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|registration| {
            registration.get("method").and_then(Value::as_str)
                == Some("workspace/didChangeWatchedFiles")
        })
        .flat_map(|registration| {
            registration
                .pointer("/registerOptions/watchers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(WatchRegistration::from_watcher)
                .collect::<Vec<_>>()
        })
        .collect()
}

pub(crate) fn should_exclude_workspace_path(path: &Path) -> bool {
    if is_high_priority_file_for_any_server(path) {
        return false;
    }

    if path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .any(is_excluded_dir_name)
    {
        return true;
    }

    let Some(file_name) = lowercase_file_name(path) else {
        return true;
    };
    if file_name.starts_with('.') && language_support_for_path(path).is_none() {
        return true;
    }
    if file_name.ends_with('~') {
        return true;
    }

    if let Some(extension) = lowercase_extension(path) {
        if is_excluded_extension(extension.as_str()) {
            return true;
        }
    }

    std::fs::metadata(path)
        .map(|metadata| metadata.len() > MAX_TRACKED_FILE_BYTES)
        .unwrap_or(false)
}

fn is_high_priority_file_for_any_server(path: &Path) -> bool {
    [
        &TYPESCRIPT_SPEC,
        &GO_SPEC,
        &RUST_SPEC,
        &PYTHON_SPEC,
        &CLANGD_SPEC,
        &JAVA_SPEC,
    ]
    .into_iter()
    .any(|spec| is_high_priority_file(path, spec))
}

fn is_excluded_dir_name(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "node_modules"
            | "dist"
            | "build"
            | "out"
            | "bin"
            | ".idea"
            | ".vscode"
            | ".cache"
            | "coverage"
            | "target"
            | "vendor"
    )
}

fn is_excluded_extension(extension: &str) -> bool {
    matches!(
        extension,
        "swp"
            | "swo"
            | "tmp"
            | "temp"
            | "bak"
            | "log"
            | "o"
            | "so"
            | "dylib"
            | "dll"
            | "a"
            | "exe"
            | "lock"
            | "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "bmp"
            | "ico"
            | "zip"
            | "tar"
            | "gz"
            | "rar"
            | "7z"
            | "pdf"
            | "mp3"
            | "mp4"
            | "mov"
            | "wav"
            | "wasm"
    )
}

pub(crate) fn collect_workspace_events(event: &Event) -> Vec<(PathBuf, WorkspaceWatchEvent)> {
    match &event.kind {
        EventKind::Create(CreateKind::Any)
        | EventKind::Create(CreateKind::File)
        | EventKind::Create(CreateKind::Folder)
        | EventKind::Create(CreateKind::Other) => event
            .paths
            .iter()
            .cloned()
            .map(|path| (path, WorkspaceWatchEvent::Created))
            .collect(),
        EventKind::Modify(ModifyKind::Name(_)) => {
            if event.paths.len() >= 2 {
                vec![
                    (event.paths[0].clone(), WorkspaceWatchEvent::Deleted),
                    (
                        event.paths[event.paths.len() - 1].clone(),
                        WorkspaceWatchEvent::Created,
                    ),
                ]
            } else {
                event
                    .paths
                    .iter()
                    .cloned()
                    .map(|path| (path, WorkspaceWatchEvent::Changed))
                    .collect()
            }
        }
        EventKind::Modify(_) | EventKind::Any => event
            .paths
            .iter()
            .cloned()
            .map(|path| (path, WorkspaceWatchEvent::Changed))
            .collect(),
        EventKind::Remove(RemoveKind::Any)
        | EventKind::Remove(RemoveKind::File)
        | EventKind::Remove(RemoveKind::Folder)
        | EventKind::Remove(RemoveKind::Other) => event
            .paths
            .iter()
            .cloned()
            .map(|path| (path, WorkspaceWatchEvent::Deleted))
            .collect(),
        _ => Vec::new(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum WorkspaceWatchEvent {
    Created,
    Changed,
    Deleted,
}

impl WorkspaceWatchEvent {
    pub(crate) fn lsp_kind(self) -> u8 {
        match self {
            Self::Created => 1,
            Self::Changed => 2,
            Self::Deleted => 3,
        }
    }

    pub(crate) fn merge(self, other: Self) -> Self {
        use WorkspaceWatchEvent::{Changed, Created, Deleted};
        match (self, other) {
            (Deleted, _) | (_, Deleted) => Deleted,
            (Created, _) | (_, Created) => Created,
            _ => Changed,
        }
    }
}

fn normalize_path_text(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{ModifyKind, RenameMode};
    use serde_json::json;

    #[test]
    fn spec_detection_covers_special_files() {
        assert_eq!(
            server_spec_for_path(Path::new("src/app.ts")).unwrap().id,
            "typescript"
        );
        assert_eq!(server_spec_for_path(Path::new("go.mod")).unwrap().id, "go");
        assert_eq!(
            server_spec_for_path(Path::new(".bashrc")).unwrap().id,
            "shell"
        );
        assert_eq!(
            server_spec_for_path(Path::new("Containerfile.prod"))
                .unwrap()
                .id,
            "dockerfile"
        );
    }

    #[test]
    fn managed_paths_follow_installer_layouts() {
        let root = Path::new("/tmp/lsp-cache");
        assert_eq!(
            managed_executable_path(root, &TYPESCRIPT_SPEC),
            root.join("typescript/node_modules/.bin/typescript-language-server")
        );
        assert_eq!(
            managed_executable_path(root, &TOML_SPEC),
            root.join("toml/bin/taplo")
        );
    }

    #[test]
    fn build_install_args_embed_prefix() {
        let root = Path::new("/tmp/install");
        assert_eq!(
            build_npm_install_args(root, &["yaml-language-server"]),
            vec![
                "install",
                "--prefix",
                "/tmp/install",
                "--no-save",
                "yaml-language-server"
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
                "/tmp/install",
                "python-lsp-server"
            ]
        );
        assert_eq!(
            build_cargo_install_args(root, "taplo-cli"),
            vec!["install", "--root", "/tmp/install", "--locked", "taplo-cli"]
        );
    }

    #[test]
    fn workspace_rename_becomes_delete_and_create() {
        let event = Event {
            kind: EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            paths: vec![PathBuf::from("old.ts"), PathBuf::from("new.ts")],
            attrs: Default::default(),
        };
        let changes = collect_workspace_events(&event);
        assert_eq!(
            changes,
            vec![
                (PathBuf::from("old.ts"), WorkspaceWatchEvent::Deleted),
                (PathBuf::from("new.ts"), WorkspaceWatchEvent::Created)
            ]
        );
    }

    #[test]
    fn watch_registration_handles_relative_patterns() {
        let params = json!({
            "registrations": [{
                "method": "workspace/didChangeWatchedFiles",
                "registerOptions": {
                    "watchers": [{
                        "globPattern": {
                            "baseUri": "file:///tmp/workspace/src",
                            "pattern": "**/*.ts"
                        },
                        "kind": 2
                    }]
                }
            }]
        });

        let registrations = extract_watch_registrations(&params);
        assert_eq!(registrations.len(), 1);
        assert!(registrations[0].matches(
            Path::new("/tmp/workspace"),
            Path::new("/tmp/workspace/src/main.ts"),
            WorkspaceWatchEvent::Changed
        ));
        assert!(!registrations[0].matches(
            Path::new("/tmp/workspace"),
            Path::new("/tmp/workspace/src/main.rs"),
            WorkspaceWatchEvent::Changed
        ));
    }

    #[test]
    fn supported_dotfiles_are_not_excluded() {
        assert!(!should_exclude_workspace_path(Path::new(".bashrc")));
        assert!(should_exclude_workspace_path(Path::new(".env")));
    }

    #[test]
    fn preload_is_reserved_for_servers_that_benefit_from_it() {
        let dir = tempfile::tempdir().unwrap();
        let ts_path = dir.path().join("app.ts");
        let go_path = dir.path().join("main.go");
        let readme_path = dir.path().join("README.md");
        std::fs::write(&ts_path, "export const value = 1;\n").unwrap();
        std::fs::write(&go_path, "package main\n").unwrap();
        std::fs::write(&readme_path, "# docs\n").unwrap();

        assert!(should_preload_path(&ts_path, &TYPESCRIPT_SPEC));
        assert!(!should_preload_path(&go_path, &GO_SPEC));
        assert!(!should_preload_path(&readme_path, &TYPESCRIPT_SPEC));
    }

    #[test]
    fn preload_candidates_apply_limit_after_sorted_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let zeta = dir.path().join("zeta.ts");
        let middle = dir.path().join("middle.ts");
        let alpha = dir.path().join("alpha.ts");
        std::fs::write(&zeta, "export const zeta = 1;\n").unwrap();
        std::fs::write(&middle, "export const middle = 1;\n").unwrap();
        std::fs::write(&alpha, "export const alpha = 1;\n").unwrap();

        let candidates = collect_preload_candidates(dir.path(), &TYPESCRIPT_SPEC, 2);

        assert_eq!(candidates, vec![alpha, middle]);
    }
}
