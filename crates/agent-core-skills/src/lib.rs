//! Skill package loading and cataloging.
//!
//! Skills are treated as core runtime context assets. This crate loads skill
//! packages and exposes stable catalog metadata; dynamic behavior should come
//! from hooks or explicit file reads, not client-side heuristics.

mod catalog;
mod frontmatter;
mod loader;

pub use catalog::*;
pub use frontmatter::*;
pub use loader::*;
