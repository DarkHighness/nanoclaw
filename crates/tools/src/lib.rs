//! Local tool abstraction plus the default built-in tool set.
//!
//! The default surface is intentionally small: `read`, `write`, `edit`,
//! provider-specific patch tooling (`apply_patch` on OpenAI-oriented surfaces,
//! `patch` on Anthropic-oriented surfaces), `glob`, `grep`, `list`,
//! `view_image`, `exec_command`, and `write_stdin`. Non-essential tool
//! bundles should be exposed through explicit Cargo features instead of
//! silently expanding the default runtime surface.

#[cfg(feature = "agentic-tools")]
pub mod agentic;
pub mod annotations;
#[cfg(feature = "code-intel")]
pub mod code_intel;
pub mod context;
mod error;
mod file_activity;
pub mod fs;
pub mod mcp_adapter;
pub mod permissions;
pub mod process;
pub mod registry;
pub mod schema;
pub mod user_input;
#[cfg(feature = "web-tools")]
pub mod web;

#[cfg(feature = "agentic-tools")]
pub use agentic::*;
pub use annotations::*;
#[cfg(feature = "code-intel")]
pub use code_intel::*;
pub use context::*;
pub use error::*;
pub use file_activity::*;
pub use fs::*;
pub use mcp_adapter::*;
pub use permissions::*;
pub use process::*;
pub use registry::*;
pub use schema::*;
pub use user_input::*;
#[cfg(feature = "web-tools")]
pub use web::*;
