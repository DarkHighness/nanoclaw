//! Self-improvement control-plane substrate for `nanoclaw`.
//!
//! The long-lived model in this crate is the `artifact` ledger: runtime
//! observation yields signals, signals yield tasks, tasks yield self-regression
//! corpus entries, and isolated evaluation determines whether a new artifact
//! version is safe to promote.

pub mod archive;
pub mod benchmark;
pub mod candidate;
pub mod corpus;
pub mod critic;
pub mod experiment;
mod git_gate;
pub mod improve;
pub mod miner;
pub mod promotion;
pub mod replay;
pub mod rollback;
pub mod runner_trace;
pub mod signals;
pub mod tasks;
pub mod worktree_runner;

pub use archive::*;
pub use benchmark::*;
pub use candidate::*;
pub use corpus::*;
pub use critic::*;
pub use experiment::*;
pub use improve::*;
pub use miner::*;
pub use promotion::*;
pub use replay::*;
pub use rollback::*;
pub use runner_trace::*;
pub use signals::*;
pub use tasks::*;
pub use worktree_runner::*;
