//! Shared inference substrate for embedding and lightweight LLM-assisted ranking.
//!
//! This crate intentionally contains only provider-agnostic inference building
//! blocks: service config models, typed expansion/rerank payload contracts, and
//! HTTP clients that map those contracts onto OpenAI-compatible endpoints.

mod config;
mod error;
mod http;
mod models;
mod traits;

pub use config::*;
pub use error::*;
pub use http::*;
pub use models::*;
pub use traits::*;

#[cfg(test)]
mod tests {
    use super::{ExpandedQueryKind, parse_expanded_queries};

    #[test]
    fn parse_expanded_queries_reads_typed_lines() {
        let parsed = parse_expanded_queries(
            "lex: redis sentinel failover\nvec: how to configure redis failover\nhyde: Setup uses sentinel quorum",
        )
        .unwrap();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].kind, ExpandedQueryKind::Lex);
        assert_eq!(parsed[1].kind, ExpandedQueryKind::Vec);
        assert_eq!(parsed[2].kind, ExpandedQueryKind::Hyde);
    }

    #[test]
    fn parse_expanded_queries_handles_json_wrapped_output() {
        let parsed = parse_expanded_queries(
            r#"```json
{"queries":[{"type":"lex","query":"cache invalidation strategy"},{"type":"vec","query":"how cache invalidation works"}]}
```"#,
        )
        .unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].kind, ExpandedQueryKind::Lex);
        assert_eq!(parsed[1].kind, ExpandedQueryKind::Vec);
    }
}
