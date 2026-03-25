//! Convenience umbrella crate for the workspace.
//!
//! Use this crate when a single dependency is more convenient than wiring the
//! narrower crates yourself. For strict minimal embeddings, prefer depending on
//! the specific core crates you need rather than treating the whole workspace as
//! one inseparable runtime unit.

mod builder;
mod prelude;

pub use agent_core_mcp as mcp;
pub use agent_core_rig as rig;
pub use agent_core_runtime as runtime;
pub use agent_core_skills as skills;
pub use agent_core_store as store;
pub use agent_core_tools as tools;
pub use agent_core_types as types;

pub use builder::*;
pub use prelude::*;
