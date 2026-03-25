//! Provider adapter layer built on top of `rig-core`.
//!
//! This crate is not the runtime itself. It translates between the core
//! runtime contracts and concrete provider APIs such as OpenAI and Anthropic.

mod backend;
mod capabilities;
mod mapping;

pub use backend::*;
pub use capabilities::*;
pub use mapping::*;
