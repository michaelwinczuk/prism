//! Convenience re-exports for common Prism types.
//!
//! ```rust
//! use prism_core::prelude::*;
//! ```

pub use crate::error::{PrismError, PrismResult};
pub use crate::mesh::{
    AgentEndpoint, AgentResponse, ConsensusConfig, ConsensusResult, ConsensusStrategy, VotingMesh,
};
pub use crate::checkpoint::{
    Checkpoint, CheckpointStore, FileStore, MemoryStore, Message, MessageRole,
    ReplayEngine, ReplayOutcome, ReplayResult,
};
