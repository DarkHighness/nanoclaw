//! Evaluation and verifier substrate for Meta Agent experiments.
//!
//! This crate is intentionally seeded with a narrow skeleton first so later
//! work can fill in evaluator contracts, benchmark packs, and active verifier
//! implementations without reworking workspace boundaries.

pub mod builtins;
pub mod context;
pub mod evaluator;
pub mod registry;
pub mod result;

pub use builtins::*;
pub use context::*;
pub use evaluator::*;
pub use registry::*;
pub use result::*;
