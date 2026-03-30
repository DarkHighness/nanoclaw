//! Core agent turn loop and runtime orchestration.
//!
//! This crate is provider-agnostic. It owns the append-only transcript loop,
//! tool execution boundary, hook lifecycle, approval boundary, and compaction
//! interfaces without depending on any one UI or provider adapter.

mod agent_mailbox;
mod agent_session_manager;
mod approval;
mod backend;
mod builder;
mod compaction;
mod control;
mod error;
mod hooks;
mod host_runtime;
mod loop_detection;
mod observer;
mod permissions;
mod runtime;
mod session;
#[cfg(feature = "agentic-tools")]
#[path = "subagent_impl.rs"]
mod subagent;
mod transcript;
mod write_lease;

pub use agent_mailbox::*;
pub use agent_session_manager::*;
pub use approval::*;
pub use backend::*;
pub use builder::*;
pub use compaction::*;
pub use control::*;
pub use error::*;
pub use hooks::*;
pub use host_runtime::*;
pub use loop_detection::*;
pub use observer::*;
pub use permissions::*;
pub use runtime::*;
pub use session::*;
#[cfg(feature = "agentic-tools")]
pub use subagent::*;
pub use transcript::*;
pub use write_lease::*;
