//! Run persistence and replay support.
//!
//! Storage is intentionally separated from the core turn loop so embeddings can
//! choose in-memory, file-backed, or custom run stores without changing runtime
//! semantics.

mod file;
mod memory;
mod replay;
mod traits;

pub use file::*;
pub use memory::*;
pub use replay::*;
pub use traits::*;
