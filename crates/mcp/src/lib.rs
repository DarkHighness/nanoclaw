//! MCP integration layer.
//!
//! This crate is an edge integration, not part of the smallest runtime loop.
//! It connects external MCP servers and adapts their tools, prompts, and
//! resources into the same boundaries used by the core runtime.

mod bridge;
mod catalog;
mod client;
mod config;
mod error;

pub use bridge::*;
pub use catalog::*;
pub use client::*;
pub use config::*;
pub use error::*;
