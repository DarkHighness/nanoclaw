//! Convenience umbrella crate for the workspace.
//!
//! Use this crate when a single dependency is more convenient than wiring the
//! narrower crates yourself. For strict minimal embeddings, prefer depending on
//! the specific core crates you need rather than treating the whole workspace as
//! one inseparable runtime unit.

mod builder;
mod plugin_boot;
mod prelude;

pub use mcp;
pub use memory;
pub use plugins;
pub use provider;
pub use runtime;
pub use skills;
pub use store;
pub use tools;
pub use types;

pub use builder::*;
pub use plugin_boot::*;
pub use prelude::*;
