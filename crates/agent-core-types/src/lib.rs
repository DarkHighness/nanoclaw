//! Provider-agnostic runtime data contracts.
//!
//! This crate is part of the smallest useful runtime closure. It defines the
//! shared message, tool, hook, id, and event types used everywhere else.

mod error;
mod event;
mod hook;
mod id;
mod message;
mod tool;

pub use error::*;
pub use event::*;
pub use hook::*;
pub use id::*;
pub use message::*;
pub use tool::*;
