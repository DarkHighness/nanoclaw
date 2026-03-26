use crate::{Result, ToolExecutionContext};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Serialize;
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
}
