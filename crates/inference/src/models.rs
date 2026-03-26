use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExpandedQueryKind {
    Lex,
    Vec,
    Hyde,
    Hybrid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpandedQuery {
    pub kind: ExpandedQueryKind,
    pub query: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RerankJudgment {
    pub relevant: bool,
    pub confidence: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct RerankDocument {
    pub title: String,
    pub path: String,
    pub text: String,
}
