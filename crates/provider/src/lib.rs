//! First-party provider adapter layer for the substrate.
//!
//! This crate is intentionally narrow. It owns only the provider transports and
//! protocol mapping that the substrate actually needs, instead of inheriting a
//! larger third-party abstraction surface as part of the public contract.

mod anthropic;
mod backend;
mod capabilities;
mod error;
mod mapping;
mod openai;

pub(crate) use anthropic::*;
pub use backend::*;
pub use capabilities::*;
pub use error::*;
pub use mapping::*;
pub use openai::*;
