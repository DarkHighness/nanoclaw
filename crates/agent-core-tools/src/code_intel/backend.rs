use crate::{Result, ToolExecutionContext};
use async_trait::async_trait;
use serde::Serialize;
use std::fmt::{Display, Formatter};
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
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

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct CodeLocation {
    pub path: String,
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CodeSymbol {
    pub name: String,
    pub kind: CodeSymbolKind,
    pub location: CodeLocation,
    pub signature: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CodeReference {
    pub symbol: String,
    pub location: CodeLocation,
    pub line_text: String,
    pub is_definition: bool,
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
        symbol: &str,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> Result<Vec<CodeSymbol>>;

    async fn references(
        &self,
        symbol: &str,
        include_declaration: bool,
        limit: usize,
        ctx: &ToolExecutionContext,
    ) -> Result<Vec<CodeReference>>;
}
