use crate::ToolExecutionContext;
use crate::annotations::mcp_tool_annotations;
use crate::code_intel::{
    CodeIntelBackend, CodeReference, CodeSymbol, WorkspaceTextCodeIntelBackend,
};
use crate::fs::resolve_tool_path_against_workspace_root;
use crate::registry::Tool;
use crate::{Result, ToolError};
use async_trait::async_trait;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use types::{MessagePart, ToolCallId, ToolOrigin, ToolOutputMode, ToolResult, ToolSpec};

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
    pub symbol: String,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct CodeReferencesInput {
    pub symbol: String,
    pub limit: Option<usize>,
    #[serde(default)]
    pub include_declaration: Option<bool>,
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
        ToolSpec {
            name: "code_symbol_search".into(),
            description: "Search for symbol declarations across the workspace. Returns symbol kind, path, line/column, and declaration signature.".to_string(),
            input_schema: serde_json::to_value(schema_for!(CodeSymbolSearchInput))
                .expect("code_symbol_search schema"),
            output_mode: ToolOutputMode::Text,
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("Code Symbol Search", true, false, true, false),
        }
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
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "code_symbol_search".into(),
            parts: vec![MessagePart::text(text)],
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
        ToolSpec {
            name: "code_document_symbols".into(),
            description: "List symbol declarations in one source file. Returns symbol kind, line/column, and declaration signature.".to_string(),
            input_schema: serde_json::to_value(schema_for!(CodeDocumentSymbolsInput))
                .expect("code_document_symbols schema"),
            output_mode: ToolOutputMode::Text,
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("Code Document Symbols", true, false, true, false),
        }
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
        let resolved = resolve_tool_path_against_workspace_root(
            &input.path,
            ctx.effective_root(),
            ctx.container_workdir.as_deref(),
        )?;
        if ctx.workspace_only {
            ctx.assert_path_allowed(&resolved)?;
        }
        let symbols = self
            .backend
            .document_symbols(resolved.as_path(), limit, ctx)
            .await?;
        let text = format_symbols_output(
            "code_document_symbols",
            &[
                ("path".to_string(), input.path),
                ("limit".to_string(), limit.to_string()),
                ("backend".to_string(), self.backend.name().to_string()),
            ],
            &symbols,
            "No declarations were found in the requested document.",
        );
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "code_document_symbols".into(),
            parts: vec![MessagePart::text(text)],
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
        ToolSpec {
            name: "code_definitions".into(),
            description: "Resolve declaration locations for a symbol name across the workspace."
                .to_string(),
            input_schema: serde_json::to_value(schema_for!(CodeDefinitionsInput))
                .expect("code_definitions schema"),
            output_mode: ToolOutputMode::Text,
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("Code Definitions", true, false, true, false),
        }
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: CodeDefinitionsInput = serde_json::from_value(arguments)?;
        let symbol = input.symbol.trim();
        if symbol.is_empty() {
            return Err(ToolError::invalid(
                "code_definitions requires a non-empty symbol",
            ));
        }
        let limit = clamp_limit(input.limit);
        let symbols = self.backend.definitions(symbol, limit, ctx).await?;
        let text = format_symbols_output(
            "code_definitions",
            &[
                ("symbol".to_string(), symbol.to_string()),
                ("limit".to_string(), limit.to_string()),
                ("backend".to_string(), self.backend.name().to_string()),
            ],
            &symbols,
            "No matching declaration was found.",
        );
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "code_definitions".into(),
            parts: vec![MessagePart::text(text)],
            metadata: Some(json!({
                "symbol": symbol,
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
        ToolSpec {
            name: "code_references".into(),
            description: "Find lexical symbol references across the workspace with optional declaration inclusion.".to_string(),
            input_schema: serde_json::to_value(schema_for!(CodeReferencesInput))
                .expect("code_references schema"),
            output_mode: ToolOutputMode::Text,
            origin: ToolOrigin::Local,
            annotations: mcp_tool_annotations("Code References", true, false, true, false),
        }
    }

    async fn execute(
        &self,
        call_id: ToolCallId,
        arguments: Value,
        ctx: &ToolExecutionContext,
    ) -> Result<ToolResult> {
        let external_call_id = types::CallId::from(&call_id);
        let input: CodeReferencesInput = serde_json::from_value(arguments)?;
        let symbol = input.symbol.trim();
        if symbol.is_empty() {
            return Err(ToolError::invalid(
                "code_references requires a non-empty symbol",
            ));
        }
        let limit = clamp_limit(input.limit);
        let include_declaration = input.include_declaration.unwrap_or(false);
        let references = self
            .backend
            .references(symbol, include_declaration, limit, ctx)
            .await?;
        let text = format_references_output(
            &[
                ("symbol".to_string(), symbol.to_string()),
                ("limit".to_string(), limit.to_string()),
                (
                    "include_declaration".to_string(),
                    include_declaration.to_string(),
                ),
                ("backend".to_string(), self.backend.name().to_string()),
            ],
            &references,
        );
        Ok(ToolResult {
            id: call_id,
            call_id: external_call_id,
            tool_name: "code_references".into(),
            parts: vec![MessagePart::text(text)],
            metadata: Some(json!({
                "symbol": symbol,
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
        CodeIntelBackend, CodeLocation, CodeReference, CodeSymbol, CodeSymbolKind,
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
            _symbol: &str,
            _limit: usize,
            _ctx: &ToolExecutionContext,
        ) -> Result<Vec<CodeSymbol>> {
            self.workspace_symbols("Engine", 1, _ctx).await
        }

        async fn references(
            &self,
            symbol: &str,
            include_declaration: bool,
            _limit: usize,
            _ctx: &ToolExecutionContext,
        ) -> Result<Vec<CodeReference>> {
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
    }
}
