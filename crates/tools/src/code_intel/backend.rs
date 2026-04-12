use crate::{Result, ToolExecutionContext};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CodeSymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    Class,
    Interface,
    TypeAlias,
    Module,
    Constant,
    Variable,
    Unknown,
}

impl CodeSymbolKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Trait => "trait",
            Self::Class => "class",
            Self::Interface => "interface",
            Self::TypeAlias => "type_alias",
            Self::Module => "module",
            Self::Constant => "constant",
            Self::Variable => "variable",
            Self::Unknown => "unknown",
        }
    }
}

impl Display for CodeSymbolKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, JsonSchema)]
pub struct CodeLocation {
    pub path: String,
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct CodeSymbol {
    pub name: String,
    pub kind: CodeSymbolKind,
    pub location: CodeLocation,
    pub signature: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct CodeReference {
    pub symbol: String,
    pub location: CodeLocation,
    pub line_text: String,
    pub is_definition: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CodeSearchMatchKind {
    Symbol,
    Text,
}

impl CodeSearchMatchKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Symbol => "symbol",
            Self::Text => "text",
        }
    }
}

impl Display for CodeSearchMatchKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct CodeSearchMatch {
    pub kind: CodeSearchMatchKind,
    pub location: CodeLocation,
    pub line_text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_kind: Option<CodeSymbolKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct CodeHover {
    pub contents: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<CodeLocation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CodeDiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
    Unknown,
}

impl CodeDiagnosticSeverity {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Information => "information",
            Self::Hint => "hint",
            Self::Unknown => "unknown",
        }
    }
}

impl Display for CodeDiagnosticSeverity {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CodeDiagnosticSource {
    Lsp,
    Lexical,
    BuildOutput,
}

impl CodeDiagnosticSource {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lsp => "lsp",
            Self::Lexical => "lexical",
            Self::BuildOutput => "build_output",
        }
    }
}

impl Display for CodeDiagnosticSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct CodeDiagnostic {
    pub location: CodeLocation,
    pub severity: CodeDiagnosticSeverity,
    pub message: String,
    pub source: CodeDiagnosticSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CodeCallHierarchyDirection {
    Incoming,
    Outgoing,
}

impl CodeCallHierarchyDirection {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Incoming => "incoming",
            Self::Outgoing => "outgoing",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
pub struct CodeCallHierarchyEntry {
    pub name: String,
    pub kind: CodeSymbolKind,
    pub location: CodeLocation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub call_site_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CodeNavigationTarget {
    Symbol(String),
    Position {
        path: PathBuf,
        display_path: String,
        line: usize,
        column: usize,
    },
}

impl CodeNavigationTarget {
    #[must_use]
    pub fn symbol(symbol: impl Into<String>) -> Self {
        Self::Symbol(symbol.into())
    }

    #[must_use]
    pub fn mode_label(&self) -> &'static str {
        match self {
            Self::Symbol(_) => "symbol",
            Self::Position { .. } => "position",
        }
    }

    #[must_use]
    pub fn symbol_name(&self) -> Option<&str> {
        match self {
            Self::Symbol(symbol) => Some(symbol.as_str()),
            Self::Position { .. } => None,
        }
    }
}

#[async_trait]
pub trait CodeIntelBackend: Send + Sync {
    /// Stable backend identifier for metadata and host-level auditing.
    fn name(&self) -> &'static str;

    async fn search(
        &self,
        _query: &str,
        _path_prefix: Option<&str>,
        _limit: usize,
        _ctx: &ToolExecutionContext,
    ) -> Result<Vec<CodeSearchMatch>> {
        Ok(Vec::new())
    }

    async fn workspace_symbols(
        &self,
        query: &str,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> Result<Vec<CodeSymbol>>;

    async fn document_symbols(
        &self,
        path: &Path,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> Result<Vec<CodeSymbol>>;

    async fn definitions(
        &self,
        target: &CodeNavigationTarget,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> Result<Vec<CodeSymbol>>;

    async fn references(
        &self,
        target: &CodeNavigationTarget,
        include_declaration: bool,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> Result<Vec<CodeReference>>;

    async fn hover(
        &self,
        _target: &CodeNavigationTarget,
        _ctx: &ToolExecutionContext,
    ) -> Result<Option<CodeHover>> {
        Ok(None)
    }

    async fn implementations(
        &self,
        _target: &CodeNavigationTarget,
        _limit: usize,
        _ctx: &ToolExecutionContext,
    ) -> Result<Vec<CodeSymbol>> {
        Ok(Vec::new())
    }

    async fn call_hierarchy(
        &self,
        _target: &CodeNavigationTarget,
        _direction: CodeCallHierarchyDirection,
        _limit: usize,
        _ctx: &ToolExecutionContext,
    ) -> Result<Vec<CodeCallHierarchyEntry>> {
        Ok(Vec::new())
    }

    async fn diagnostics(
        &self,
        _path: Option<&Path>,
        _limit: usize,
        _ctx: &ToolExecutionContext,
    ) -> Result<Vec<CodeDiagnostic>> {
        Ok(Vec::new())
    }
}
