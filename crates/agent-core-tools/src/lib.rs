//! Local tool abstraction plus the default built-in tool set.
//!
//! The default surface is intentionally small: `read`, `write`, `edit`,
//! `patch`, `glob`, `grep`, `list`, and `bash`. Non-essential tool bundles should be exposed
//! through explicit Cargo features instead of silently expanding the default
//! runtime surface.

#[cfg(feature = "agentic-tools")]
pub mod agentic;
pub mod annotations;
#[cfg(feature = "code-intel")]
pub mod code_intel;
pub mod context;
pub mod fs;
pub mod mcp_adapter;
pub mod process;
pub mod registry;
pub mod schema;
#[cfg(feature = "web-tools")]
pub mod web;

#[cfg(feature = "agentic-tools")]
pub use agentic::*;
pub use annotations::*;
#[cfg(feature = "code-intel")]
pub use code_intel::*;
pub use context::*;
pub use fs::*;
pub use mcp_adapter::*;
pub use process::*;
pub use registry::*;
pub use schema::*;
#[cfg(feature = "web-tools")]
pub use web::*;
