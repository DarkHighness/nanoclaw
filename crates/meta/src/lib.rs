//! Meta Agent control-plane substrate.
//!
//! This crate starts as a skeletal workspace member so follow-up slices can add
//! candidate generation, experiment orchestration, promotion, and rollback
//! logic without another workspace-seeding change.

pub mod archive;
pub mod benchmark;
pub mod candidate;
pub mod corpus;
pub mod critic;
pub mod experiment;
pub mod improve;
pub mod miner;
pub mod promotion;
pub mod replay;
pub mod rollback;
pub mod signals;
pub mod tasks;

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
pub use signals::*;
pub use tasks::*;
