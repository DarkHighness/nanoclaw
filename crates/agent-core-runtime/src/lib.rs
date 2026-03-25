//! Core agent turn loop and runtime orchestration.
//!
//! This crate is provider-agnostic. It owns the append-only transcript loop,
//! tool execution boundary, hook lifecycle, approval boundary, and compaction
//! interfaces without depending on any one UI or provider adapter.

mod approval;
mod backend;
mod builder;
mod compaction;
mod control;
mod hooks;
mod loop_detection;
mod observer;
mod runtime;
mod session;
#[cfg(feature = "agentic-tools")]
mod subagent;
mod transcript;

pub use approval::*;
pub use backend::*;
pub use builder::*;
pub use compaction::*;
pub use control::*;
pub use hooks::*;
pub use loop_detection::*;
pub use observer::*;
pub use runtime::*;
pub use session::*;
#[cfg(feature = "agentic-tools")]
pub use subagent::*;
pub use transcript::*;
