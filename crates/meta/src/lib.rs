//! Meta Agent control-plane substrate.
//!
//! This crate starts as a skeletal workspace member so follow-up slices can add
//! candidate generation, experiment orchestration, promotion, and rollback
//! logic without another workspace-seeding change.

pub mod benchmark;
pub mod candidate;
pub mod critic;
pub mod experiment;
pub mod improve;
pub mod miner;
pub mod promotion;
pub mod rollback;
pub mod signals;
pub mod tasks;

pub use benchmark::*;
pub use candidate::*;
pub use critic::*;
pub use experiment::*;
pub use improve::*;
pub use miner::*;
pub use promotion::*;
pub use rollback::*;
pub use signals::*;
pub use tasks::*;
