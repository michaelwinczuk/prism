//! # Prism Core
//!
//! A minimal production reliability layer for agentic AI systems.
//!
//! Prism provides three core capabilities:
//!
//! 1. **VotingMesh** — Spawn N agents with different models, require consensus
//!    before accepting output.
//! 2. **Checkpoint/Replay** — Snapshot agent state, replay from checkpoint for
//!    validation and recovery.
//! 3. **Python Bindings** — (feature-gated behind `python`) Expose the full API
//!    to Python via PyO3.
//!
//! ## Performance Characteristics
//!
//! - Binary size: <10MB in release mode
//! - Startup time: <50ms
//! - Minimal dependencies for fast compilation
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use prism_core::mesh::{VotingMesh, ConsensusConfig, ConsensusStrategy, AgentEndpoint, AgentResponse};
//! use async_trait::async_trait;
//!
//! struct MyAgent { model: String }
//!
//! #[async_trait]
//! impl AgentEndpoint for MyAgent {
//!     async fn generate(&self, prompt: &str) -> Result<AgentResponse, prism_core::error::PrismError> {
//!         Ok(AgentResponse {
//!             content: format!("Response from {}", self.model),
//!             confidence: 0.95,
//!             model_id: self.model.clone(),
//!             metadata: Default::default(),
//!         })
//!     }
//! }
//! ```

pub mod error;
pub mod mesh;
pub mod checkpoint;
pub mod prelude;

#[cfg(feature = "python")]
pub mod python;

// Re-export primary public types at crate root for convenience.
pub use error::PrismError;
pub use mesh::{AgentEndpoint, AgentResponse, ConsensusConfig, ConsensusResult, ConsensusStrategy, VotingMesh};
pub use checkpoint::{
    Checkpoint, CheckpointStore, FileStore, MemoryStore, Message, MessageRole,
    ReplayEngine, ReplayOutcome, ReplayResult,
};