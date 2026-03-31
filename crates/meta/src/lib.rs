//! Meta Agent control-plane substrate.
//!
//! This crate starts as a skeletal workspace member so follow-up slices can add
//! candidate generation, experiment orchestration, promotion, and rollback
//! logic without another workspace-seeding change.

pub mod benchmark;
pub mod candidate;
pub mod critic;
pub mod experiment;
pub mod promotion;
pub mod rollback;

pub use benchmark::*;
pub use candidate::*;
pub use critic::*;
pub use experiment::*;
pub use promotion::*;
pub use rollback::*;
