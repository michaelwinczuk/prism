//! PyO3 bindings for Prism Core.
//!
//! Feature-gated behind the `python` feature flag. Build with:
//!
//! ```bash
//! maturin develop --features python
//! ```
//!
//! Then in Python:
//!
//! ```python
//! import prism_core
//! ```

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;

use std::collections::HashMap;
use std::sync::Arc;

use crate::checkpoint::{Checkpoint, CheckpointStore, MemoryStore, Message, MessageRole};
use crate::error::PrismError;
use crate::mesh::{AgentEndpoint, AgentResponse, ConsensusConfig, ConsensusStrategy, VotingMesh};

use async_trait::async_trait;
use tokio::runtime::Runtime;

// ---------------------------------------------------------------------------
// Helper: get or create a Tokio runtime
// ---------------------------------------------------------------------------

/// Cached Tokio runtime for Python bindings.
/// Created once on first use, reused for all subsequent calls.
static RUNTIME: std::sync::OnceLock<Runtime> = std::sync::OnceLock::new();

fn get_runtime() -> PyResult<&'static Runtime> {
    RUNTIME.get_or_try_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| PyRuntimeError::new_err(format!("Failed to create Tokio runtime: {e}")))
    })
}

fn prism_err_to_py(e: PrismError) -> PyErr {
    PyRuntimeError::new_err(format!("{e}"))
}

// ---------------------------------------------------------------------------
// PyCallableAgent: wraps a Python callable as an AgentEndpoint
// ---------------------------------------------------------------------------

/// An agent backed by a Python callable.
///
/// The callable must accept a single string argument (the prompt) and return
/// a dict with keys: `content` (str), `confidence` (float), `model_id` (str).
struct PyCallableAgent {
    callable: PyObject,
    agent_id: String,
}

#[async_trait]
impl AgentEndpoint for PyCallableAgent {
    async fn generate(&self, prompt: &str) -> Result<AgentResponse, PrismError> {
        // We need to call back into Python, which requires the GIL.
        // Since this is called from a Tokio task, we acquire the GIL here.
        let prompt = prompt.to_string();
        let callable = self.callable.clone();

        Python::with_gil(|py| {
            let result = callable
                .call1(py, (prompt,))
                .map_err(|e| PrismError::AgentError {
                    agent_id: self.agent_id.clone(),
                    message: format!("Python callable raised: {e}"),
                })?;

            let dict = result
                .downcast_bound::<pyo3::types::PyDict>(py)
                .map_err(|_| PrismError::AgentError {
                    agent_id: self.agent_id.clone(),
                    message: "Agent callable must return a dict".to_string(),
                })?;

            let agent_id = self.agent_id.clone();
            let extract_field = |key: &str| -> Result<pyo3::Bound<'_, pyo3::PyAny>, PrismError> {
                dict.get_item(key)
                    .map_err(|e| PrismError::AgentError {
                        agent_id: agent_id.clone(),
                        message: format!("Error accessing '{}': {}", key, e),
                    })?
                    .ok_or_else(|| PrismError::AgentError {
                        agent_id: agent_id.clone(),
                        message: format!("Missing '{}' key", key),
                    })
            };

            let content: String =
                extract_field("content")?
                    .extract()
                    .map_err(|e| PrismError::AgentError {
                        agent_id: self.agent_id.clone(),
                        message: format!("'content' must be a string: {e}"),
                    })?;

            let confidence: f64 =
                extract_field("confidence")?
                    .extract()
                    .map_err(|e| PrismError::AgentError {
                        agent_id: self.agent_id.clone(),
                        message: format!("'confidence' must be a float: {e}"),
                    })?;

            let model_id: String =
                extract_field("model_id")?
                    .extract()
                    .map_err(|e| PrismError::AgentError {
                        agent_id: self.agent_id.clone(),
                        message: format!("'model_id' must be a string: {e}"),
                    })?;

            Ok(AgentResponse {
                content,
                confidence,
                model_id,
                metadata: HashMap::new(),
            })
        })
    }

    fn agent_id(&self) -> String {
        self.agent_id.clone()
    }
}

// ---------------------------------------------------------------------------
// PyVotingMesh
// ---------------------------------------------------------------------------

/// Python wrapper for the VotingMesh.
///
/// Usage:
/// ```python
/// mesh = PyVotingMesh(strategy="majority", min_confidence=0.5, timeout_ms=30000)
/// mesh.add_agent("agent-1", my_callable)
/// result = mesh.run("What is 2+2?")
/// print(result["content"])
/// ```
#[pyclass(name = "VotingMesh")]
pub struct PyVotingMesh {
    inner: VotingMesh,
}

#[pymethods]
impl PyVotingMesh {
    /// Create a new VotingMesh.
    ///
    /// Args:
    ///     strategy: "majority", "unanimous", or "weighted" (default: "majority")
    ///     min_confidence: minimum confidence threshold (default: 0.0)
    ///     timeout_ms: timeout in milliseconds (default: 30000)
    #[new]
    #[pyo3(signature = (strategy="majority", min_confidence=0.0, timeout_ms=30000))]
    fn new(strategy: &str, min_confidence: f64, timeout_ms: u64) -> PyResult<Self> {
        let strategy = match strategy {
            "majority" => ConsensusStrategy::Majority,
            "unanimous" => ConsensusStrategy::Unanimous,
            "weighted" => ConsensusStrategy::Weighted,
            other => {
                return Err(PyValueError::new_err(format!(
                    "Unknown strategy '{}'. Use 'majority', 'unanimous', or 'weighted'",
                    other
                )));
            }
        };

        let config = ConsensusConfig {
            strategy,
            min_confidence,
            timeout_ms,
        };

        Ok(Self {
            inner: VotingMesh::new(config),
        })
    }

    /// Add an agent to the mesh.
    ///
    /// Args:
    ///     agent_id: unique identifier for this agent
    ///     callable: a Python callable that takes a prompt (str) and returns
    ///               a dict with keys: content (str), confidence (float), model_id (str)
    fn add_agent(&mut self, agent_id: &str, callable: PyObject) -> PyResult<()> {
        let agent = PyCallableAgent {
            callable,
            agent_id: agent_id.to_string(),
        };
        self.inner.add_agent(agent);
        Ok(())
    }

    /// Return the number of agents in the mesh.
    fn agent_count(&self) -> usize {
        self.inner.agent_count()
    }

    /// Run all agents on the given prompt and return the consensus result.
    ///
    /// Returns a dict with keys:
    ///   - content (str): the chosen response
    ///   - confidence (float): consensus confidence
    ///   - agreement_ratio (float): fraction of agents that agreed
    ///   - model_id (str): model that produced the chosen response
    ///   - total_responses (int): number of agents that responded
    ///   - failed_agents (int): number of agents that failed
    fn run(&self, py: Python<'_>, prompt: &str) -> PyResult<PyObject> {
        let rt = get_runtime()?;
        let result = rt
            .block_on(self.inner.run(prompt))
            .map_err(prism_err_to_py)?;

        let dict = pyo3::types::PyDict::new_bound(py);
        dict.set_item("content", &result.chosen.content)?;
        dict.set_item("confidence", result.confidence)?;
        dict.set_item("agreement_ratio", result.agreement_ratio)?;
        dict.set_item("model_id", &result.chosen.model_id)?;
        dict.set_item("total_responses", result.total_responses)?;
        dict.set_item("failed_agents", result.failed_agents)?;

        Ok(dict.into())
    }
}

// ---------------------------------------------------------------------------
// PyCheckpointManager
// ---------------------------------------------------------------------------

/// Python wrapper for checkpoint management (in-memory store).
///
/// Usage:
/// ```python
/// mgr = CheckpointManager()
/// cp_id = mgr.create("my-mission")
/// mgr.add_message(cp_id, "user", "Hello")
/// mgr.save(cp_id)
/// data = mgr.load(cp_id)
/// ```
#[pyclass(name = "CheckpointManager")]
pub struct PyCheckpointManager {
    store: Arc<MemoryStore>,
    /// In-flight checkpoints that haven't been saved yet.
    drafts: HashMap<String, Checkpoint>,
}

#[pymethods]
impl PyCheckpointManager {
    /// Create a new CheckpointManager with an in-memory store.
    #[new]
    fn new() -> Self {
        Self {
            store: Arc::new(MemoryStore::new()),
            drafts: HashMap::new(),
        }
    }

    /// Create a new checkpoint draft and return its ID.
    fn create(&mut self, mission_id: &str) -> String {
        let cp = Checkpoint::new(mission_id);
        let id = cp.id.clone();
        self.drafts.insert(id.clone(), cp);
        id
    }

    /// Add a message to a draft checkpoint.
    ///
    /// Args:
    ///     checkpoint_id: the checkpoint ID
    ///     role: "user", "assistant", "system", or "tool"
    ///     content: the message text
    fn add_message(&mut self, checkpoint_id: &str, role: &str, content: &str) -> PyResult<()> {
        let cp = self.drafts.get_mut(checkpoint_id).ok_or_else(|| {
            PyValueError::new_err(format!(
                "Checkpoint '{}' not found in drafts",
                checkpoint_id
            ))
        })?;

        let role = match role {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            "system" => MessageRole::System,
            "tool" => MessageRole::Tool,
            other => {
                return Err(PyValueError::new_err(format!(
                    "Unknown role '{}'. Use 'user', 'assistant', 'system', or 'tool'",
                    other
                )));
            }
        };

        cp.add_message(Message::new(role, content));
        Ok(())
    }

    /// Save a draft checkpoint to the store.
    fn save(&mut self, checkpoint_id: &str) -> PyResult<()> {
        let cp = self.drafts.remove(checkpoint_id).ok_or_else(|| {
            PyValueError::new_err(format!(
                "Checkpoint '{}' not found in drafts",
                checkpoint_id
            ))
        })?;

        let rt = get_runtime()?;
        rt.block_on(self.store.save(&cp)).map_err(prism_err_to_py)?;
        Ok(())
    }

    /// Load a checkpoint from the store and return it as a JSON string.
    fn load(&self, checkpoint_id: &str) -> PyResult<String> {
        let rt = get_runtime()?;
        let cp = rt
            .block_on(self.store.load(checkpoint_id))
            .map_err(prism_err_to_py)?;
        cp.to_json().map_err(prism_err_to_py)
    }

    /// List all checkpoint IDs, optionally filtered by mission_id.
    #[pyo3(signature = (mission_id=None))]
    fn list(&self, mission_id: Option<&str>) -> PyResult<Vec<String>> {
        let rt = get_runtime()?;
        rt.block_on(self.store.list(mission_id))
            .map_err(prism_err_to_py)
    }

    /// Delete a checkpoint from the store.
    fn delete(&self, checkpoint_id: &str) -> PyResult<()> {
        let rt = get_runtime()?;
        rt.block_on(self.store.delete(checkpoint_id))
            .map_err(prism_err_to_py)
    }
}

// ---------------------------------------------------------------------------
// Module registration
// ---------------------------------------------------------------------------

/// Register the prism_core Python module.
#[pymodule]
pub fn prism_core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyVotingMesh>()?;
    m.add_class::<PyCheckpointManager>()?;
    Ok(())
}
