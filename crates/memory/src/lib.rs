//! Memory retrieval substrate for workspace Markdown memories.
//!
//! This crate keeps memory retrieval independent from run-store persistence:
//! run-store tracks append-only runtime events, while memory retrieval reads
//! operator-curated Markdown files (`MEMORY.md`, `memory/**/*.md`, and explicit
//! extras) and exposes lookup tools for model turns.

mod backend;
mod config;
mod corpus;
mod error;
mod lexical_index;
mod memory_core;
mod memory_embed;
mod retrieval_policy;
mod runtime_exports;
mod state;
mod tools;
mod vector_store;

pub use backend::*;
pub use config::*;
pub use corpus::*;
pub use error::*;
pub use memory_core::*;
pub use memory_embed::*;
pub use runtime_exports::*;
pub use state::*;
pub use tools::*;
