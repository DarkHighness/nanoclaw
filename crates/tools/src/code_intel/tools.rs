use crate::ToolExecutionContext;
use crate::annotations::{builtin_tool_spec, tool_approval_profile};
use crate::code_intel::{
    CodeIntelBackend, CodeNavigationTarget, CodeReference, CodeSymbol,
    WorkspaceTextCodeIntelBackend,
};
use crate::fs::resolve_tool_path_against_workspace_root;
use crate::registry::Tool;
use crate::{Result, ToolError};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use types::{MessagePart, ToolCallId, ToolOutputMode, ToolResult, ToolSpec};

const DEFAULT_RESULT_LIMIT: usize = 32;
const MAX_RESULT_LIMIT: usize = 200;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct CodeSymbolSearchInput {
    pub query: String,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct CodeDocumentSymbolsInput {
    pub path: String,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct CodeDefinitionsInput {
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<usize>,
    #[serde(default)]
    pub column: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct CodeReferencesInput {
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<usize>,
    #[serde(default)]
    pub column: Option<usize>,
    pub limit: Option<usize>,
    #[serde(default)]
    pub include_declaration: Option<bool>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct CodeSymbolSearchToolOutput {
    query: String,
    limit: usize,
    backend: String,
    result_count: usize,
    symbols: Vec<CodeSymbol>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct CodeDocumentSymbolsToolOutput {
    requested_path: String,
    resolved_path: String,
    limit: usize,
    backend: String,
    result_count: usize,
    symbols: Vec<CodeSymbol>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct CodeDefinitionsToolOutput {
    symbol: String,
    limit: usize,
    backend: String,
    result_count: usize,
    definitions: Vec<CodeSymbol>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct CodeReferencesToolOutput {
    symbol: String,
    limit: usize,
    include_declaration: bool,
    backend: String,
    result_count: usize,
    references: Vec<CodeReference>,
}

#[derive(Clone)]
pub struct CodeSymbolSearchTool {
    backend: Arc<dyn CodeIntelBackend>,
}

#[derive(Clone)]
pub struct CodeDocumentSymbolsTool {
    backend: Arc<dyn CodeIntelBackend>,
}

#[derive(Clone)]
pub struct CodeDefinitionsTool {
    backend: Arc<dyn CodeIntelBackend>,
}

#[derive(Clone)]
pub struct CodeReferencesTool {
    backend: Arc<dyn CodeIntelBackend>,
}

impl Default for CodeSymbolSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for CodeDocumentSymbolsTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for CodeDefinitionsTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for CodeReferencesTool {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeSymbolSearchTool {
    #[must_use]
    pub fn new() -> Self {
        Self::with_backend(Arc::new(WorkspaceTextCodeIntelBackend::new()))
    }

    #[must_use]
    pub fn with_backend(backend: Arc<dyn CodeIntelBackend>) -> Self {
        Self { backend }
    }
}

impl CodeDocumentSymbolsTool {
    #[must_use]
    pub fn new() -> Self {
        Self::with_backend(Arc::new(WorkspaceTextCodeIntelBackend::new()))
    }

    #[must_use]
    pub fn with_backend(backend: Arc<dyn CodeIntelBackend>) -> Self {
        Self { backend }
    }
}

impl CodeDefinitionsTool {
    #[must_use]
    pub fn new() -> Self {
        Self::with_backend(Arc::new(WorkspaceTextCodeIntelBackend::new()))
    }

    #[must_use]
    pub fn with_backend(backend: Arc<dyn CodeIntelBackend>) -> Self {
        Self { backend }
    }
}

impl CodeReferencesTool {
    #[must_use]
    pub fn new() -> Self {
        Self::with_backend(Arc::new(WorkspaceTextCodeIntelBackend::new()))
    }

    #[must_use]
    pub fn with_backend(backend: Arc<dyn CodeIntelBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl Tool for CodeSymbolSearchTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "code_symbol_search",
            "Search for symbol declarations across the workspace. Returns symbol kind, path, line/column, and declaration signature.",
            serde_json::to_value(schema_for!(CodeSymbolSearchInput))
                .expect("code_symbol_search schema"),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, true, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(CodeSymbolSearchToolOutput))
                .expect("code_symbol_search output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: CodeSymbolSearchInput = serde_json::from_value(arguments)?;
        let query = input.query.trim();
        if query.is_empty() {
            return Err(ToolError::invalid(
                "code_symbol_search requires a non-empty query",
            ));
        }
        let limit = clamp_limit(input.limit);
        let symbols = self.backend.workspace_symbols(query, limit, ctx).await?;
        let text = format_symbols_output(
            "code_symbol_search",
            &[
                ("query".to_string(), query.to_string()),
                ("limit".to_string(), limit.to_string()),
                ("backend".to_string(), self.backend.name().to_string()),
            ],
            &symbols,
            "No symbols matched the current query.",
        );
        let structured_output = CodeSymbolSearchToolOutput {
            query: query.to_string(),
            limit,
            backend: self.backend.name().to_string(),
            result_count: symbols.len(),
            symbols: symbols.clone(),
        };
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "code_symbol_search".into(),
            parts: vec![MessagePart::text(text)],
            attachments: Vec::new(),
            structured_content: Some(
                serde_json::to_value(structured_output)
                    .expect("code_symbol_search structured output"),
            ),
            continuation: None,
            metadata: Some(json!({
                "query": query,
                "limit": limit,
                "backend": self.backend.name(),
                "result_count": symbols.len(),
                "symbols": symbols.iter().map(symbol_to_json).collect::<Vec<_>>(),
            })),
            is_error: false,
        })
    }
}

#[async_trait]
impl Tool for CodeDocumentSymbolsTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "code_document_symbols",
            "List symbol declarations in one source file. Returns symbol kind, line/column, and declaration signature.",
            serde_json::to_value(schema_for!(CodeDocumentSymbolsInput))
                .expect("code_document_symbols schema"),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, true, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(CodeDocumentSymbolsToolOutput))
                .expect("code_document_symbols output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: CodeDocumentSymbolsInput = serde_json::from_value(arguments)?;
        let limit = clamp_limit(input.limit);
        let requested_path = input.path.clone();
        let resolved = resolve_tool_path_against_workspace_root(
            &input.path,
            ctx.effective_root(),
            ctx.container_workdir.as_deref(),
        )?;
        ctx.assert_path_read_allowed(&resolved)?;
        let symbols = self
            .backend
            .document_symbols(resolved.as_path(), limit, ctx)
            .await?;
        let text = format_symbols_output(
            "code_document_symbols",
            &[
                ("path".to_string(), requested_path.clone()),
                ("limit".to_string(), limit.to_string()),
                ("backend".to_string(), self.backend.name().to_string()),
            ],
            &symbols,
            "No declarations were found in the requested document.",
        );
        let structured_output = CodeDocumentSymbolsToolOutput {
            requested_path,
            resolved_path: resolved.display().to_string(),
            limit,
            backend: self.backend.name().to_string(),
            result_count: symbols.len(),
            symbols: symbols.clone(),
        };
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "code_document_symbols".into(),
            parts: vec![MessagePart::text(text)],
            attachments: Vec::new(),
            structured_content: Some(
                serde_json::to_value(structured_output)
                    .expect("code_document_symbols structured output"),
            ),
            continuation: None,
            metadata: Some(json!({
                "path": resolved,
                "limit": limit,
                "backend": self.backend.name(),
                "result_count": symbols.len(),
                "symbols": symbols.iter().map(symbol_to_json).collect::<Vec<_>>(),
            })),
            is_error: false,
        })
    }
}

#[async_trait]
impl Tool for CodeDefinitionsTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "code_definitions",
            "Resolve declaration locations either from a symbol name or from a file position (`path` + `line` + optional `column`) for semantic backends such as LSP.",
            serde_json::to_value(schema_for!(CodeDefinitionsInput))
                .expect("code_definitions schema"),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, true, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(CodeDefinitionsToolOutput))
                .expect("code_definitions output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: CodeDefinitionsInput = serde_json::from_value(arguments)?;
        let target = resolve_navigation_target(
            input.symbol.as_deref(),
            input.path.as_deref(),
            input.line,
            input.column,
            ctx,
        )?;
        let limit = clamp_limit(input.limit);
        let symbols = self.backend.definitions(&target, limit, ctx).await?;
        let text = format_symbols_output(
            "code_definitions",
            &format_navigation_tags(&target, limit, self.backend.name()),
            &symbols,
            "No matching declaration was found.",
        );
        let structured_output = CodeDefinitionsToolOutput {
            symbol: format_navigation_target_label(&target),
            limit,
            backend: self.backend.name().to_string(),
            result_count: symbols.len(),
            definitions: symbols.clone(),
        };
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "code_definitions".into(),
            parts: vec![MessagePart::text(text)],
            attachments: Vec::new(),
            structured_content: Some(
                serde_json::to_value(structured_output)
                    .expect("code_definitions structured output"),
            ),
            continuation: None,
            metadata: Some(json!({
                "target": navigation_target_to_json(&target),
                "limit": limit,
                "backend": self.backend.name(),
                "result_count": symbols.len(),
                "definitions": symbols.iter().map(symbol_to_json).collect::<Vec<_>>(),
            })),
            is_error: false,
        })
    }
}

#[async_trait]
impl Tool for CodeReferencesTool {
    fn spec(&self) -> ToolSpec {
        builtin_tool_spec(
            "code_references",
            "Find symbol references either from a symbol name or from a file position (`path` + `line` + optional `column`). Semantic backends use the position form for true LSP references.",
            serde_json::to_value(schema_for!(CodeReferencesInput))
                .expect("code_references schema"),
            ToolOutputMode::Text,
            tool_approval_profile(true, false, true, false),
        )
        .with_output_schema(
            serde_json::to_value(schema_for!(CodeReferencesToolOutput))
                .expect("code_references output schema"),
        )
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: CodeReferencesInput = serde_json::from_value(arguments)?;
        let target = resolve_navigation_target(
            input.symbol.as_deref(),
            input.path.as_deref(),
            input.line,
            input.column,
            ctx,
        )?;
        let limit = clamp_limit(input.limit);
        let include_declaration = input.include_declaration.unwrap_or(false);
        let references = self
            .backend
            .references(&target, include_declaration, limit, ctx)
            .await?;
        let text = format_references_output(
            &{
                let mut tags = format_navigation_tags(&target, limit, self.backend.name());
                tags.push((
                    "include_declaration".to_string(),
                    include_declaration.to_string(),
                ));
                tags
            },
            &references,
        );
        let structured_output = CodeReferencesToolOutput {
            symbol: format_navigation_target_label(&target),
            limit,
            include_declaration,
            backend: self.backend.name().to_string(),
            result_count: references.len(),
            references: references.clone(),
        };
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "code_references".into(),
            parts: vec![MessagePart::text(text)],
            attachments: Vec::new(),
            structured_content: Some(
                serde_json::to_value(structured_output).expect("code_references structured output"),
            ),
            continuation: None,
            metadata: Some(json!({
                "target": navigation_target_to_json(&target),
                "limit": limit,
                "include_declaration": include_declaration,
                "backend": self.backend.name(),
                "result_count": references.len(),
                "references": references.iter().map(reference_to_json).collect::<Vec<_>>(),
            })),
            is_error: false,
        })
    }
}

fn clamp_limit(limit: Option<usize>) -> usize {
    limit
        .unwrap_or(DEFAULT_RESULT_LIMIT)
        .clamp(1, MAX_RESULT_LIMIT)
}

fn resolve_navigation_target(
    symbol: Option<&str>,
    path: Option<&str>,
    line: Option<usize>,
    column: Option<usize>,
    ctx: &ToolExecutionContext,
) -> Result<CodeNavigationTarget> {
    if let Some(path) = path.map(str::trim).filter(|value| !value.is_empty()) {
        let Some(line) = line else {
            return Err(ToolError::invalid(
                "position-based code-intel queries require line when path is provided",
            ));
        };
        let resolved = resolve_tool_path_against_workspace_root(
            path,
            ctx.effective_root(),
            ctx.container_workdir.as_deref(),
        )?;
        ctx.assert_path_read_allowed(&resolved)?;
        let display_path = resolved
            .strip_prefix(ctx.effective_root())
            .unwrap_or(&resolved)
            .to_string_lossy()
            .replace('\\', "/");
        return Ok(CodeNavigationTarget::Position {
            path: resolved,
            display_path,
            line,
            column: column.unwrap_or(1).max(1),
        });
    }

    let symbol = symbol
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ToolError::invalid("code-intel navigation requires either symbol or path+line(+column)")
        })?;
    Ok(CodeNavigationTarget::symbol(symbol))
}

fn format_navigation_tags(
    target: &CodeNavigationTarget,
    limit: usize,
    backend_name: &str,
) -> Vec<(String, String)> {
    let mut tags = vec![
        ("query_mode".to_string(), target.mode_label().to_string()),
        ("limit".to_string(), limit.to_string()),
        ("backend".to_string(), backend_name.to_string()),
    ];
    match target {
        CodeNavigationTarget::Symbol(symbol) => {
            tags.push(("symbol".to_string(), symbol.clone()));
        }
        CodeNavigationTarget::Position {
            display_path,
            line,
            column,
            ..
        } => {
            tags.push(("path".to_string(), display_path.clone()));
            tags.push(("line".to_string(), line.to_string()));
            tags.push(("column".to_string(), column.to_string()));
        }
    }
    tags
}

fn navigation_target_to_json(target: &CodeNavigationTarget) -> Value {
    match target {
        CodeNavigationTarget::Symbol(symbol) => json!({
            "mode": "symbol",
            "symbol": symbol,
        }),
        CodeNavigationTarget::Position {
            path,
            display_path,
            line,
            column,
        } => json!({
            "mode": "position",
            "path": path,
            "display_path": display_path,
            "line": line,
            "column": column,
        }),
    }
}

fn format_navigation_target_label(target: &CodeNavigationTarget) -> String {
    match target {
        CodeNavigationTarget::Symbol(symbol) => symbol.clone(),
        CodeNavigationTarget::Position {
            display_path,
            line,
            column,
            ..
        } => format!("{display_path}:{line}:{column}"),
    }
}

fn format_symbols_output(
    name: &str,
    tags: &[(String, String)],
    symbols: &[CodeSymbol],
    empty_message: &str,
) -> String {
    let mut lines = vec![format!(
        "[{name} {} results={}]",
        tags.iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(" "),
        symbols.len()
    )];
    if symbols.is_empty() {
        lines.push(empty_message.to_string());
        return lines.join("\n");
    }
    for (index, symbol) in symbols.iter().enumerate() {
        lines.push(format!(
            "{}. {} [{}] {}:{}:{}",
            index + 1,
            symbol.name,
            symbol.kind,
            symbol.location.path,
            symbol.location.line,
            symbol.location.column
        ));
        if let Some(signature) = &symbol.signature {
            lines.push(format!("   sig> {signature}"));
        }
    }
    lines.join("\n")
}

fn format_references_output(tags: &[(String, String)], references: &[CodeReference]) -> String {
    let mut lines = vec![format!(
        "[code_references {} results={}]",
        tags.iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(" "),
        references.len()
    )];
    if references.is_empty() {
        lines.push("No references matched the requested symbol.".to_string());
        return lines.join("\n");
    }
    for (index, reference) in references.iter().enumerate() {
        let suffix = if reference.is_definition {
            " [definition]"
        } else {
            ""
        };
        lines.push(format!(
            "{}. {}:{}:{}{}",
            index + 1,
            reference.location.path,
            reference.location.line,
            reference.location.column,
            suffix
        ));
        lines.push(format!("   line> {}", reference.line_text));
    }
    lines.join("\n")
}

fn symbol_to_json(symbol: &CodeSymbol) -> Value {
    json!({
        "name": symbol.name,
        "kind": symbol.kind.as_str(),
        "path": symbol.location.path,
        "line": symbol.location.line,
        "column": symbol.location.column,
        "signature": symbol.signature,
    })
}

fn reference_to_json(reference: &CodeReference) -> Value {
    json!({
        "symbol": reference.symbol,
        "path": reference.location.path,
        "line": reference.location.line,
        "column": reference.location.column,
        "line_text": reference.line_text,
        "is_definition": reference.is_definition,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        CodeDefinitionsTool, CodeDocumentSymbolsTool, CodeReferencesTool, CodeSymbolSearchTool,
        Tool,
    };
    use crate::Result;
    use crate::ToolExecutionContext;
    use crate::code_intel::{
        CodeIntelBackend, CodeLocation, CodeNavigationTarget, CodeReference, CodeSymbol,
        CodeSymbolKind,
    };
    use async_trait::async_trait;
    use serde_json::json;
    use std::path::Path;
    use std::sync::Arc;
    use types::ToolCallId;

    #[derive(Clone, Debug)]
    struct StubBackend;

    #[async_trait]
    impl CodeIntelBackend for StubBackend {
        fn name(&self) -> &'static str {
            "stub_backend"
        }

        async fn workspace_symbols(
            &self,
            query: &str,
            _limit: usize,
            _ctx: &ToolExecutionContext,
        ) -> Result<Vec<CodeSymbol>> {
            if query == "missing" {
                return Ok(Vec::new());
            }
            Ok(vec![CodeSymbol {
                name: "Engine".to_string(),
                kind: CodeSymbolKind::Struct,
                location: CodeLocation {
                    path: "src/lib.rs".to_string(),
                    line: 10,
                    column: 12,
                },
                signature: Some("pub struct Engine;".to_string()),
            }])
        }

        async fn document_symbols(
            &self,
            _path: &Path,
            _limit: usize,
            _ctx: &ToolExecutionContext,
        ) -> Result<Vec<CodeSymbol>> {
            self.workspace_symbols("Engine", 1, _ctx).await
        }

        async fn definitions(
            &self,
            _target: &CodeNavigationTarget,
            _limit: usize,
            _ctx: &ToolExecutionContext,
        ) -> Result<Vec<CodeSymbol>> {
            self.workspace_symbols("Engine", 1, _ctx).await
        }

        async fn references(
            &self,
            target: &CodeNavigationTarget,
            include_declaration: bool,
            _limit: usize,
            _ctx: &ToolExecutionContext,
        ) -> Result<Vec<CodeReference>> {
            let symbol = target.symbol_name().unwrap_or("Engine");
            let mut refs = vec![CodeReference {
                symbol: symbol.to_string(),
                location: CodeLocation {
                    path: "src/lib.rs".to_string(),
                    line: 10,
                    column: 12,
                },
                line_text: "pub struct Engine;".to_string(),
                is_definition: true,
            }];
            if include_declaration {
                refs.push(CodeReference {
                    symbol: symbol.to_string(),
                    location: CodeLocation {
                        path: "src/main.rs".to_string(),
                        line: 22,
                        column: 9,
                    },
                    line_text: "let _ = Engine {};".to_string(),
                    is_definition: false,
                });
            }
            Ok(refs)
        }
    }

    fn context(root: &std::path::Path) -> ToolExecutionContext {
        ToolExecutionContext {
            workspace_root: root.to_path_buf(),
            workspace_only: true,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn symbol_search_tool_formats_stub_result() {
        let dir = tempfile::tempdir().unwrap();
        let tool = CodeSymbolSearchTool::with_backend(Arc::new(StubBackend));
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "query": "Engine"
                }),
                &context(dir.path()),
            )
            .await
            .unwrap();
        let text = result.text_content();
        assert!(text.contains("[code_symbol_search"));
        assert!(text.contains("Engine [struct]"));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["query"], "Engine");
        assert_eq!(structured["symbols"][0]["name"], "Engine");
    }

    #[tokio::test]
    async fn document_symbols_tool_resolves_workspace_paths() {
        let dir = tempfile::tempdir().unwrap();
        let sample = dir.path().join("src/lib.rs");
        std::fs::create_dir_all(sample.parent().unwrap()).unwrap();
        std::fs::write(&sample, "pub struct Engine;\n").unwrap();

        let tool = CodeDocumentSymbolsTool::with_backend(Arc::new(StubBackend));
        let result = tool
            .execute(
                ToolCallId::new(),
                json!({
                    "path": "src/lib.rs"
                }),
                &context(dir.path()),
            )
            .await
            .unwrap();
        assert!(result.text_content().contains("results=1"));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["requested_path"], "src/lib.rs");
        assert_eq!(structured["symbols"][0]["location"]["path"], "src/lib.rs");
    }

    #[tokio::test]
    async fn definitions_and_references_tools_emit_expected_headers() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(StubBackend);
        let definitions = CodeDefinitionsTool::with_backend(backend.clone())
            .execute(
                ToolCallId::new(),
                json!({
                    "symbol": "Engine"
                }),
                &context(dir.path()),
            )
            .await
            .unwrap();
        let references = CodeReferencesTool::with_backend(backend)
            .execute(
                ToolCallId::new(),
                json!({
                    "symbol": "Engine",
                    "include_declaration": true
                }),
                &context(dir.path()),
            )
            .await
            .unwrap();

        assert!(definitions.text_content().contains("[code_definitions"));
        assert!(references.text_content().contains("[code_references"));
        assert!(references.text_content().contains("[definition]"));
        let definitions_structured = definitions.structured_content.unwrap();
        assert_eq!(definitions_structured["definitions"][0]["name"], "Engine");
        let references_structured = references.structured_content.unwrap();
        assert_eq!(references_structured["include_declaration"], true);
        assert_eq!(
            references_structured["references"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn definitions_tool_accepts_position_queries() {
        let dir = tempfile::tempdir().unwrap();
        let sample = dir.path().join("src/lib.rs");
        std::fs::create_dir_all(sample.parent().unwrap()).unwrap();
        std::fs::write(&sample, "pub struct Engine;\n").unwrap();

        let result = CodeDefinitionsTool::with_backend(Arc::new(StubBackend))
            .execute(
                ToolCallId::new(),
                json!({
                    "path": "src/lib.rs",
                    "line": 1,
                    "column": 12
                }),
                &context(dir.path()),
            )
            .await
            .unwrap();

        assert!(result.text_content().contains("query_mode=position"));
        assert!(result.text_content().contains("path=src/lib.rs"));
    }
}
