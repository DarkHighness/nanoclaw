//! Memory retrieval substrate for workspace Markdown memories.
//!
//! This crate keeps memory retrieval independent from session-store persistence:
//! session-store tracks append-only session events, while memory retrieval reads
//! operator-curated Markdown files (`MEMORY.md`, `memory/**/*.md`, and explicit
//! extras) and exposes lookup tools for model turns.

mod auto_index;
mod backend;
mod config;
mod corpus;
mod error;
mod lexical_index;
mod managed_files;
mod memory_core;
#[cfg(feature = "memory-embed")]
mod memory_embed;
mod promotion;
mod retention;
mod retrieval_policy;
mod runtime_exports;
mod state;
mod tools;
#[cfg(feature = "memory-embed")]
mod vector_store;

pub use backend::*;
pub use config::*;
pub use corpus::*;
pub use error::*;
pub use memory_core::*;
#[cfg(feature = "memory-embed")]
pub use memory_embed::*;
pub use runtime_exports::*;
pub use state::*;
pub use tools::*;
