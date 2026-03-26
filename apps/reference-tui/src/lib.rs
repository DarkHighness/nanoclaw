//! Reference terminal shell for the workspace runtime.
//!
//! This crate is an operator-facing shell over the same runtime APIs used by
//! the rest of the workspace. It is not the core agent framework itself, is not
//! part of the workspace default foundation path, carries its config layer
//! privately, and can be removed without changing the base runtime crates.

mod app;
mod boot;
mod command;
mod config;
mod render;

pub use app::*;
pub use boot::*;
pub use command::*;
pub use render::*;
