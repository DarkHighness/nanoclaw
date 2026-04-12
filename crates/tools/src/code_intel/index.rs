use crate::ToolExecutionContext;
use crate::code_intel::{
    CodeIntelBackend, CodeLocation, CodeNavigationTarget, CodeReference, CodeSearchMatch,
    CodeSearchMatchKind, CodeSymbol, CodeSymbolKind,
};
use crate::{Result, ToolError};
use async_trait::async_trait;
use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const DEFAULT_MAX_FILE_BYTES: usize = 512 * 1024;
const MAX_REFERENCE_LINE_CHARS: usize = 240;

const DEFAULT_INDEXED_EXTENSIONS: &[&str] = &[
    "c", "cc", "cpp", "cs", "go", "h", "hpp", "java", "js", "jsx", "kt", "m", "mm", "php", "py",
    "rb", "rs", "swift", "ts", "tsx",
];

#[derive(Clone, Debug)]
pub struct WorkspaceTextCodeIntelBackend {
    indexed_extensions: BTreeSet<String>,
    max_file_bytes: usize,
}

impl Default for WorkspaceTextCodeIntelBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceTextCodeIntelBackend {
    #[must_use]
    pub fn new() -> Self {
        Self::with_settings(
            DEFAULT_INDEXED_EXTENSIONS
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            DEFAULT_MAX_FILE_BYTES,
        )
    }

    #[must_use]
    pub fn with_settings(indexed_extensions: BTreeSet<String>, max_file_bytes: usize) -> Self {
        Self {
            indexed_extensions,
            max_file_bytes: max_file_bytes.max(1),
        }
    }

    fn collect_workspace_symbols(&self, root: &Path) -> Result<Vec<CodeSymbol>> {
        // This backend rebuilds from disk per request so cache invalidation stays a host concern.
        // Hosts that already maintain an index can implement CodeIntelBackend with their own cache.
        let mut symbols = Vec::new();
        for path in self.workspace_files(root) {
            let source = match std::fs::read_to_string(&path) {
                Ok(source) => source,
                Err(_) => continue,
            };
            symbols.extend(self.parse_symbols_in_source(root, &path, &source));
        }
        symbols.sort_by_key(symbol_sort_key);
        Ok(symbols)
    }

    fn workspace_files(&self, root: &Path) -> Vec<PathBuf> {
        let mut builder = WalkBuilder::new(root);
        builder.standard_filters(true);
        builder.follow_links(false);
        let mut files = builder
            .build()
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.into_path())
            .filter(|path| self.should_scan_path(path))
            .collect::<Vec<_>>();
        files.sort();
        files
    }

    fn should_scan_path(&self, path: &Path) -> bool {
        if !path.is_file() {
            return false;
        }
        let Some(ext) = path.extension().and_then(|value| value.to_str()) else {
            return false;
        };
        if !self.indexed_extensions.contains(&ext.to_ascii_lowercase()) {
            return false;
        }
        match std::fs::metadata(path) {
            Ok(metadata) => metadata.len() <= self.max_file_bytes as u64,
            Err(_) => false,
        }
    }

    fn parse_symbols_in_source(&self, root: &Path, path: &Path, source: &str) -> Vec<CodeSymbol> {
        source
            .lines()
            .enumerate()
            .filter_map(|(line_idx, line)| {
                let (kind, name) = parse_symbol_from_line(line)?;
                let column = line.find(&name).map_or(1, |offset| offset + 1);
                let signature = normalize_signature(line);
                Some(CodeSymbol {
                    name,
                    kind,
                    location: CodeLocation {
                        path: display_path(root, path),
                        line: line_idx + 1,
                        column,
                    },
                    signature,
                })
            })
            .collect()
    }

    fn collect_search_matches(
        &self,
        root: &Path,
        query: &str,
        path_prefix: Option<&str>,
        limit: usize,
    ) -> Result<Vec<CodeSearchMatch>> {
        let normalized_query = query.trim();
        if normalized_query.is_empty() {
            return Ok(Vec::new());
        }
        let normalized_prefix = normalized_path_prefix(path_prefix);
        let lowered_query = normalized_query.to_ascii_lowercase();

        let mut matches = self
            .collect_workspace_symbols(root)?
            .into_iter()
            .filter(|symbol| {
                matches_search_prefix(&symbol.location.path, normalized_prefix.as_deref())
            })
            .filter(|symbol| symbol.name.to_ascii_lowercase().contains(&lowered_query))
            .map(symbol_to_search_match)
            .collect::<Vec<_>>();

        let symbol_lines = matches
            .iter()
            .map(|entry| (entry.location.path.clone(), entry.location.line))
            .collect::<HashSet<_>>();

        let pattern = build_search_query_regex(normalized_query)?;
        let candidate_limit = limit.saturating_mul(4).max(limit);
        'files: for path in self.workspace_files(root) {
            let display_path = display_path(root, &path);
            if !matches_search_prefix(&display_path, normalized_prefix.as_deref()) {
                continue;
            }
            let source = match std::fs::read_to_string(&path) {
                Ok(source) => source,
                Err(_) => continue,
            };
            for (line_idx, line) in source.lines().enumerate() {
                let Some(found) = pattern.find(line) else {
                    continue;
                };
                if symbol_lines.contains(&(display_path.clone(), line_idx + 1)) {
                    continue;
                }
                matches.push(CodeSearchMatch {
                    kind: CodeSearchMatchKind::Text,
                    location: CodeLocation {
                        path: display_path.clone(),
                        line: line_idx + 1,
                        column: found.start() + 1,
                    },
                    line_text: compact_line(line),
                    symbol_name: None,
                    symbol_kind: None,
                    signature: None,
                });
                if matches.len() >= candidate_limit {
                    break 'files;
                }
            }
        }

        matches.sort_by(|left, right| {
            code_search_match_sort_key(left, &lowered_query)
                .cmp(&code_search_match_sort_key(right, &lowered_query))
        });
        matches.truncate(limit);
        Ok(matches)
    }
}

#[async_trait]
impl CodeIntelBackend for WorkspaceTextCodeIntelBackend {
    fn name(&self) -> &'static str {
        "workspace_text_scan_v1"
    }

    async fn search(
        &self,
        query: &str,
        path_prefix: Option<&str>,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> Result<Vec<CodeSearchMatch>> {
        self.collect_search_matches(ctx.effective_root(), query, path_prefix, limit)
    }

    async fn workspace_symbols(
        &self,
        query: &str,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> Result<Vec<CodeSymbol>> {
        let normalized_query = query.trim().to_ascii_lowercase();
        let mut symbols = self.collect_workspace_symbols(ctx.effective_root())?;
        if !normalized_query.is_empty() {
            symbols.retain(|symbol| symbol.name.to_ascii_lowercase().contains(&normalized_query));
            symbols.sort_by(|left, right| {
                symbol_query_rank(left, &normalized_query)
                    .cmp(&symbol_query_rank(right, &normalized_query))
            });
        }
        symbols.truncate(limit);
        Ok(symbols)
    }

    async fn document_symbols(
        &self,
        path: &Path,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> Result<Vec<CodeSymbol>> {
        let source = std::fs::read_to_string(path).map_err(|source| {
            ToolError::invalid_state(format!(
                "failed to read source file {}: {source}",
                path.display()
            ))
        })?;
        let mut symbols = self.parse_symbols_in_source(ctx.effective_root(), path, &source);
        symbols.sort_by_key(symbol_sort_key);
        symbols.truncate(limit);
        Ok(symbols)
    }

    async fn definitions(
        &self,
        target: &CodeNavigationTarget,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> Result<Vec<CodeSymbol>> {
        let query = resolve_navigation_symbol(target)?.trim().to_string();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let all_symbols = self.collect_workspace_symbols(ctx.effective_root())?;
        let mut symbols = all_symbols
            .iter()
            .filter(|entry| entry.name == query)
            .cloned()
            .collect::<Vec<_>>();
        if symbols.is_empty() {
            let lowered = query.to_ascii_lowercase();
            symbols = all_symbols
                .into_iter()
                .filter(|entry| entry.name.to_ascii_lowercase() == lowered)
                .collect();
        }
        symbols.truncate(limit);
        Ok(symbols)
    }

    async fn references(
        &self,
        target: &CodeNavigationTarget,
        include_declaration: bool,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> Result<Vec<CodeReference>> {
        let query = resolve_navigation_symbol(target)?;
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let declaration_set = self
            .collect_workspace_symbols(ctx.effective_root())?
            .into_iter()
            .filter(|entry| entry.name == query)
            .map(|entry| {
                (
                    entry.location.path,
                    entry.location.line,
                    entry.location.column,
                )
            })
            .collect::<HashSet<_>>();
        let pattern = build_symbol_regex(&query)?;
        let mut refs = Vec::new();
        for path in self.workspace_files(ctx.effective_root()) {
            let source = match std::fs::read_to_string(&path) {
                Ok(source) => source,
                Err(_) => continue,
            };
            let display_path = display_path(ctx.effective_root(), &path);
            for (line_idx, line) in source.lines().enumerate() {
                for found in pattern.find_iter(line) {
                    let location = CodeLocation {
                        path: display_path.clone(),
                        line: line_idx + 1,
                        column: found.start() + 1,
                    };
                    let is_definition = declaration_set.contains(&(
                        location.path.clone(),
                        location.line,
                        location.column,
                    ));
                    // LSP-style "include declaration" is explicit here because this backend
                    // uses lexical matching, not semantic binding. The caller decides whether
                    // declaration sites should be part of the result set.
                    if !include_declaration && is_definition {
                        continue;
                    }
                    refs.push(CodeReference {
                        symbol: query.to_string(),
                        line_text: compact_line(line),
                        location,
                        is_definition,
                    });
                    if refs.len() >= limit {
                        return Ok(refs);
                    }
                }
            }
        }
        Ok(refs)
    }
}

fn symbol_sort_key(symbol: &CodeSymbol) -> (String, usize, usize, String, CodeSymbolKind) {
    (
        symbol.location.path.clone(),
        symbol.location.line,
        symbol.location.column,
        symbol.name.clone(),
        symbol.kind,
    )
}

fn symbol_query_rank(symbol: &CodeSymbol, query: &str) -> (u8, String, usize, usize) {
    let lowered = symbol.name.to_ascii_lowercase();
    let rank = if lowered == query {
        0
    } else if lowered.starts_with(query) {
        1
    } else {
        2
    };
    (
        rank,
        symbol.location.path.clone(),
        symbol.location.line,
        symbol.location.column,
    )
}

fn code_search_match_sort_key(
    entry: &CodeSearchMatch,
    lowered_query: &str,
) -> (u8, String, usize, usize, String) {
    let query_rank = match entry.kind {
        CodeSearchMatchKind::Symbol => {
            let lowered_name = entry
                .symbol_name
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase();
            if lowered_name == lowered_query {
                0
            } else if lowered_name.starts_with(lowered_query) {
                1
            } else {
                2
            }
        }
        CodeSearchMatchKind::Text => {
            let lowered_line = entry.line_text.to_ascii_lowercase();
            if lowered_line == lowered_query {
                4
            } else if lowered_line.contains(lowered_query) {
                5
            } else {
                6
            }
        }
    };
    (
        query_rank,
        entry.location.path.clone(),
        entry.location.line,
        entry.location.column,
        entry.symbol_name.clone().unwrap_or_default(),
    )
}

fn symbol_to_search_match(symbol: CodeSymbol) -> CodeSearchMatch {
    CodeSearchMatch {
        kind: CodeSearchMatchKind::Symbol,
        location: symbol.location,
        line_text: symbol
            .signature
            .clone()
            .unwrap_or_else(|| symbol.name.clone()),
        symbol_name: Some(symbol.name),
        symbol_kind: Some(symbol.kind),
        signature: symbol.signature,
    }
}

fn normalized_path_prefix(path_prefix: Option<&str>) -> Option<String> {
    path_prefix
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .replace('\\', "/")
                .trim_start_matches("./")
                .trim_start_matches('/')
                .to_string()
        })
}

fn matches_search_prefix(display_path: &str, path_prefix: Option<&str>) -> bool {
    path_prefix.is_none_or(|prefix| {
        display_path == prefix
            || display_path
                .strip_prefix(prefix)
                .is_some_and(|suffix| suffix.starts_with('/'))
    })
}

fn definition_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:export\s+)?(?:async\s+)?(?P<kw>fn|def|function|class|interface|struct|enum|trait|mod)\s+(?P<name>[A-Za-z_][A-Za-z0-9_$]*)",
        )
        .expect("definition regex")
    })
}

fn type_alias_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^\s*(?:pub\s+)?(?:export\s+)?type\s+(?P<name>[A-Za-z_][A-Za-z0-9_$]*)\b")
            .expect("type alias regex")
    })
}

fn variable_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"^\s*(?:pub\s+)?(?:export\s+)?(?P<kw>const|let|var)\s+(?P<name>[A-Za-z_$][A-Za-z0-9_$]*)\b",
        )
        .expect("variable regex")
    })
}

fn go_func_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^\s*func\s+(?:\([^)]*\)\s*)?(?P<name>[A-Za-z_][A-Za-z0-9_$]*)\s*\(")
            .expect("go func regex")
    })
}

fn parse_symbol_from_line(line: &str) -> Option<(CodeSymbolKind, String)> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//")
        || trimmed.starts_with('#')
        || trimmed.starts_with("/*")
        || trimmed.starts_with('*')
    {
        return None;
    }

    if let Some(captures) = definition_regex().captures(trimmed) {
        let keyword = captures.name("kw")?.as_str();
        let name = captures.name("name")?.as_str().to_owned();
        return Some((map_keyword_kind(keyword, &name), name));
    }

    if let Some(captures) = type_alias_regex().captures(trimmed) {
        let name = captures.name("name")?.as_str().to_owned();
        return Some((CodeSymbolKind::TypeAlias, name));
    }

    if let Some(captures) = variable_regex().captures(trimmed) {
        let keyword = captures.name("kw")?.as_str();
        let name = captures.name("name")?.as_str().to_owned();
        let kind = if keyword == "const" || is_all_caps_name(&name) {
            CodeSymbolKind::Constant
        } else {
            CodeSymbolKind::Variable
        };
        return Some((kind, name));
    }

    if let Some(captures) = go_func_regex().captures(trimmed) {
        let name = captures.name("name")?.as_str().to_owned();
        return Some((CodeSymbolKind::Function, name));
    }

    None
}

fn map_keyword_kind(keyword: &str, name: &str) -> CodeSymbolKind {
    match keyword {
        "fn" | "def" | "function" => CodeSymbolKind::Function,
        "class" => CodeSymbolKind::Class,
        "interface" => CodeSymbolKind::Interface,
        "struct" => CodeSymbolKind::Struct,
        "enum" => CodeSymbolKind::Enum,
        "trait" => CodeSymbolKind::Trait,
        "mod" => CodeSymbolKind::Module,
        "const" => CodeSymbolKind::Constant,
        "let" | "var" => {
            if is_all_caps_name(name) {
                CodeSymbolKind::Constant
            } else {
                CodeSymbolKind::Variable
            }
        }
        _ => CodeSymbolKind::Unknown,
    }
}

fn is_all_caps_name(value: &str) -> bool {
    let mut saw_alpha = false;
    for char in value.chars() {
        if char.is_ascii_alphabetic() {
            saw_alpha = true;
            if !char.is_ascii_uppercase() {
                return false;
            }
        }
    }
    saw_alpha
}

fn normalize_signature(line: &str) -> Option<String> {
    let normalized = line.trim();
    if normalized.is_empty() {
        return None;
    }
    Some(compact_line(normalized))
}

fn compact_line(line: &str) -> String {
    let compact = line.trim();
    let mut shortened = compact
        .chars()
        .take(MAX_REFERENCE_LINE_CHARS)
        .collect::<String>();
    if compact.chars().count() > MAX_REFERENCE_LINE_CHARS {
        shortened.push_str("...");
    }
    shortened
}

fn build_search_query_regex(query: &str) -> Result<Regex> {
    RegexBuilder::new(&regex::escape(query))
        .case_insensitive(true)
        .build()
        .map_err(|source| ToolError::invalid(format!("invalid search query `{query}`: {source}")))
}

fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn build_symbol_regex(symbol: &str) -> Result<Regex> {
    let escaped = regex::escape(symbol);
    let pattern = if symbol
        .chars()
        .all(|value| value.is_ascii_alphanumeric() || value == '_' || value == '$')
    {
        format!(r"\b{escaped}\b")
    } else {
        escaped
    };
    Regex::new(&pattern).map_err(|error| {
        ToolError::invalid(format!("invalid symbol pattern for `{symbol}`: {error}"))
    })
}

fn resolve_navigation_symbol(target: &CodeNavigationTarget) -> Result<String> {
    match target {
        CodeNavigationTarget::Symbol(symbol) => Ok(symbol.trim().to_string()),
        CodeNavigationTarget::Position {
            path, line, column, ..
        } => symbol_at_position(path, *line, *column),
    }
}

fn symbol_at_position(path: &Path, line: usize, column: usize) -> Result<String> {
    let source = std::fs::read_to_string(path).map_err(|error| {
        ToolError::invalid_state(format!(
            "failed to read source file {}: {error}",
            path.display()
        ))
    })?;
    let Some(line_text) = source.lines().nth(line.saturating_sub(1)) else {
        return Err(ToolError::invalid(format!(
            "line {line} is outside {}",
            path.display()
        )));
    };
    let chars = line_text.char_indices().collect::<Vec<_>>();
    if chars.is_empty() {
        return Err(ToolError::invalid(format!(
            "line {line} in {} is empty",
            path.display()
        )));
    }
    let requested_column = column.max(1);
    let byte_index = chars
        .iter()
        .find_map(|(offset, _)| ((*offset + 1) >= requested_column).then_some(*offset))
        .unwrap_or(line_text.len());
    let (start, end) = identifier_bounds(line_text, byte_index);
    if start == end {
        return Err(ToolError::invalid(format!(
            "no identifier found at {}:{}:{}",
            path.display(),
            line,
            column
        )));
    }
    Ok(line_text[start..end].to_string())
}

fn identifier_bounds(line: &str, byte_index: usize) -> (usize, usize) {
    let bytes = line.as_bytes();
    if bytes.is_empty() {
        return (0, 0);
    }

    let mut cursor = byte_index.min(bytes.len());
    if cursor == bytes.len() {
        cursor = bytes.len().saturating_sub(1);
    }
    if !is_identifier_byte(bytes[cursor]) {
        return (0, 0);
    }

    let mut start = cursor;
    while start > 0 && is_identifier_byte(bytes[start - 1]) {
        start -= 1;
    }

    let mut end = cursor + 1;
    while end < bytes.len() && is_identifier_byte(bytes[end]) {
        end += 1;
    }
    (start, end)
}

fn is_identifier_byte(ch: u8) -> bool {
    ch.is_ascii_alphanumeric() || ch == b'_' || ch == b'$'
}

#[cfg(test)]
mod tests {
    use super::{CodeIntelBackend, WorkspaceTextCodeIntelBackend};
    use crate::ToolExecutionContext;
    use crate::code_intel::{CodeNavigationTarget, CodeSearchMatchKind};

    fn context(root: &std::path::Path) -> ToolExecutionContext {
        ToolExecutionContext {
            workspace_root: root.to_path_buf(),
            workspace_only: true,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn workspace_symbols_index_common_declarations() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "pub struct Engine;\nfn load_file() {}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app.ts"),
            "export interface Runner {}\nexport function runTask() {}\n",
        )
        .unwrap();

        let backend = WorkspaceTextCodeIntelBackend::new();
        let symbols = backend
            .workspace_symbols("run", 16, &context(dir.path()))
            .await
            .unwrap();

        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].name, "Runner");
        assert_eq!(symbols[1].name, "runTask");
    }

    #[tokio::test]
    async fn document_symbols_only_report_requested_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let left = dir.path().join("src/left.rs");
        let right = dir.path().join("src/right.rs");
        std::fs::write(&left, "fn alpha() {}\n").unwrap();
        std::fs::write(&right, "fn beta() {}\n").unwrap();

        let backend = WorkspaceTextCodeIntelBackend::new();
        let symbols = backend
            .document_symbols(&left, 16, &context(dir.path()))
            .await
            .unwrap();

        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "alpha");
        assert_eq!(symbols[0].location.path, "src/left.rs");
    }

    #[tokio::test]
    async fn references_can_exclude_declaration_sites() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "fn compute_total() {}\nfn main() { let _ = compute_total(); }\n",
        )
        .unwrap();

        let backend = WorkspaceTextCodeIntelBackend::new();
        let without_decl = backend
            .references(
                &CodeNavigationTarget::symbol("compute_total"),
                false,
                16,
                &context(dir.path()),
            )
            .await
            .unwrap();
        let with_decl = backend
            .references(
                &CodeNavigationTarget::symbol("compute_total"),
                true,
                16,
                &context(dir.path()),
            )
            .await
            .unwrap();

        assert_eq!(without_decl.len(), 1);
        assert!(!without_decl[0].is_definition);
        assert_eq!(with_decl.len(), 2);
        assert!(with_decl.iter().any(|entry| entry.is_definition));
    }

    #[tokio::test]
    async fn position_queries_extract_identifier_under_cursor() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let path = dir.path().join("src/lib.rs");
        std::fs::write(
            &path,
            "fn compute_total() {}\nfn main() { let _ = compute_total(); }\n",
        )
        .unwrap();

        let backend = WorkspaceTextCodeIntelBackend::new();
        let target = CodeNavigationTarget::Position {
            path: path.clone(),
            display_path: "src/lib.rs".to_string(),
            line: 2,
            column: 29,
        };
        let definitions = backend
            .definitions(&target, 8, &context(dir.path()))
            .await
            .unwrap();

        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].name, "compute_total");
        assert_eq!(definitions[0].location.path, "src/lib.rs");
    }

    #[tokio::test]
    async fn code_search_returns_symbol_and_text_matches_with_path_prefix() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/runtime")).unwrap();
        std::fs::create_dir_all(dir.path().join("src/other")).unwrap();
        std::fs::write(
            dir.path().join("src/runtime/lib.rs"),
            "pub struct Engine;\nfn boot() { let _ = Engine; }\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/other/lib.rs"),
            "pub struct Worker;\nfn boot() { let _ = Engine; }\n",
        )
        .unwrap();

        let backend = WorkspaceTextCodeIntelBackend::new();
        let matches = backend
            .search("Engine", Some("src/runtime"), 8, &context(dir.path()))
            .await
            .unwrap();

        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].kind, CodeSearchMatchKind::Symbol);
        assert_eq!(matches[0].location.path, "src/runtime/lib.rs");
        assert_eq!(matches[1].kind, CodeSearchMatchKind::Text);
        assert_eq!(matches[1].location.path, "src/runtime/lib.rs");
    }
}
