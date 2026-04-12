//! Skill package loading and cataloging.
//!
//! Skills are treated as core runtime context assets. This crate loads skill
//! packages and exposes stable catalog metadata; dynamic behavior should come
//! from hooks or explicit file reads, not client-side heuristics.

mod catalog;
mod error;
mod frontmatter;
mod loader;
mod model;

pub use catalog::*;
pub use error::*;
pub use frontmatter::*;
pub use loader::*;
pub use model::*;
