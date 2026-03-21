//! Error types for the Prism crate.

use thiserror::Error;

/// Top-level error type for all Prism operations.
#[derive(Debug, Error)]
pub enum PrismError {
    /// An agent endpoint returned an error during generation.
    #[error("agent error ({agent_id}): {message}")]
    AgentError {
        /// Identifier of the agent that failed.
        agent_id: String,
        /// Human-readable error message.
        message: String,
    },

    /// Consensus could not be reached within the configured constraints.
    #[error("consensus failed: {reason}")]
    ConsensusFailure {
        /// Explanation of why consensus was not reached.
        reason: String,
    },

    /// A timeout expired before all agents responded.
    #[error("timeout after {timeout_ms}ms: {detail}")]
    Timeout {
        /// The configured timeout in milliseconds.
        timeout_ms: u64,
        /// Additional detail about what timed out.
        detail: String,
    },

    /// Checkpoint serialization or deserialization failed.
    #[error("checkpoint error: {0}")]
    Checkpoint(String),

    /// I/O error (file system, network, etc.).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Replay detected a divergence or failure.
    #[error("replay error: {0}")]
    Replay(String),

    /// No agents were configured in the voting mesh.
    #[error("no agents configured in voting mesh")]
    NoAgents,

    /// A required checkpoint was not found.
    #[error("checkpoint not found: {0}")]
    NotFound(String),
}

/// Convenience Result alias for Prism operations.
pub type PrismResult<T> = Result<T, PrismError>;
