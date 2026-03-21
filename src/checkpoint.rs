//! Checkpoint and Replay: state serialization, pluggable storage, and replay validation.
//!
//! The checkpoint module provides:
//!
//! - [`Checkpoint`] — a serializable snapshot of agent conversation state.
//! - [`CheckpointStore`] trait — pluggable storage backend.
//! - [`MemoryStore`] — in-memory store (for testing and ephemeral use).
//! - [`FileStore`] — JSON file-based persistent store.
//! - [`ReplayEngine`] — re-execute from checkpoint and compare outputs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::{PrismError, PrismResult};
use crate::mesh::{AgentResponse, VotingMesh};

// ---------------------------------------------------------------------------
// Message types
// ---------------------------------------------------------------------------

/// Role of a message participant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    /// The human user.
    User,
    /// The AI assistant / agent.
    Assistant,
    /// System instructions.
    System,
    /// A tool or function call result.
    Tool,
}

/// A single message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    /// Who sent this message.
    pub role: MessageRole,
    /// Text content of the message.
    pub content: String,
    /// Optional name or identifier of the sender.
    pub name: Option<String>,
    /// Timestamp when the message was created.
    pub timestamp: DateTime<Utc>,
}

impl Message {
    /// Create a new message with the current timestamp.
    pub fn new(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            name: None,
            timestamp: Utc::now(),
        }
    }

    /// Set the sender name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
}

// ---------------------------------------------------------------------------
// FunctionCall
// ---------------------------------------------------------------------------

/// Record of a function/tool call made during a conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionCall {
    /// Name of the function that was called.
    pub name: String,
    /// Arguments passed (as a JSON string).
    pub arguments: String,
    /// The result returned by the function.
    pub result: Option<String>,
    /// Timestamp of the call.
    pub timestamp: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Checkpoint
// ---------------------------------------------------------------------------

/// A serializable snapshot of agent conversation state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Unique identifier for this checkpoint.
    pub id: String,
    /// Mission or session identifier.
    pub mission_id: String,
    /// When this checkpoint was created.
    pub created_at: DateTime<Utc>,
    /// The full conversation history at the time of the checkpoint.
    pub conversation_history: Vec<Message>,
    /// Function calls made during the conversation.
    pub function_calls: Vec<FunctionCall>,
    /// The last agent response (the output that was checkpointed).
    pub last_response: Option<AgentResponse>,
    /// Arbitrary metadata.
    pub metadata: HashMap<String, String>,
}

impl Checkpoint {
    /// Create a new checkpoint with a generated UUID.
    pub fn new(mission_id: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            mission_id: mission_id.into(),
            created_at: Utc::now(),
            conversation_history: Vec::new(),
            function_calls: Vec::new(),
            last_response: None,
            metadata: HashMap::new(),
        }
    }

    /// Add a message to the conversation history.
    pub fn add_message(&mut self, message: Message) {
        self.conversation_history.push(message);
    }

    /// Record a function call.
    pub fn add_function_call(&mut self, call: FunctionCall) {
        self.function_calls.push(call);
    }

    /// Set the last agent response.
    pub fn set_response(&mut self, response: AgentResponse) {
        self.last_response = Some(response);
    }

    /// Serialize the checkpoint to a JSON string.
    pub fn to_json(&self) -> PrismResult<String> {
        serde_json::to_string_pretty(self).map_err(PrismError::Json)
    }

    /// Deserialize a checkpoint from a JSON string.
    pub fn from_json(json: &str) -> PrismResult<Self> {
        serde_json::from_str(json).map_err(PrismError::Json)
    }
}

// ---------------------------------------------------------------------------
// CheckpointStore trait
// ---------------------------------------------------------------------------

/// Pluggable storage backend for checkpoints.
#[async_trait]
pub trait CheckpointStore: Send + Sync {
    /// Save a checkpoint. Overwrites if the same ID already exists.
    async fn save(&self, checkpoint: &Checkpoint) -> PrismResult<()>;

    /// Load a checkpoint by its ID.
    async fn load(&self, id: &str) -> PrismResult<Checkpoint>;

    /// List all checkpoint IDs, optionally filtered by mission_id.
    async fn list(&self, mission_id: Option<&str>) -> PrismResult<Vec<String>>;

    /// Delete a checkpoint by its ID.
    async fn delete(&self, id: &str) -> PrismResult<()>;
}

// ---------------------------------------------------------------------------
// MemoryStore
// ---------------------------------------------------------------------------

/// In-memory checkpoint store backed by a `HashMap`.
///
/// Useful for testing and ephemeral sessions. Data is lost when the store
/// is dropped.
#[derive(Debug, Default)]
pub struct MemoryStore {
    data: RwLock<HashMap<String, Checkpoint>>,
}

impl MemoryStore {
    /// Create a new empty in-memory store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl CheckpointStore for MemoryStore {
    async fn save(&self, checkpoint: &Checkpoint) -> PrismResult<()> {
        let mut data = self.data.write().await;
        data.insert(checkpoint.id.clone(), checkpoint.clone());
        Ok(())
    }

    async fn load(&self, id: &str) -> PrismResult<Checkpoint> {
        let data = self.data.read().await;
        data.get(id)
            .cloned()
            .ok_or_else(|| PrismError::NotFound(id.to_string()))
    }

    async fn list(&self, mission_id: Option<&str>) -> PrismResult<Vec<String>> {
        let data = self.data.read().await;
        let ids: Vec<String> = data
            .values()
            .filter(|cp| match mission_id {
                Some(mid) => cp.mission_id == mid,
                None => true,
            })
            .map(|cp| cp.id.clone())
            .collect();
        Ok(ids)
    }

    async fn delete(&self, id: &str) -> PrismResult<()> {
        let mut data = self.data.write().await;
        data.remove(id)
            .map(|_| ())
            .ok_or_else(|| PrismError::NotFound(id.to_string()))
    }
}

// ---------------------------------------------------------------------------
// FileStore
// ---------------------------------------------------------------------------

/// File-system checkpoint store that persists each checkpoint as a JSON file.
///
/// Each checkpoint is stored as `<base_dir>/<checkpoint_id>.json`.
#[derive(Debug, Clone)]
pub struct FileStore {
    base_dir: PathBuf,
}

impl FileStore {
    /// Create a new file store rooted at the given directory.
    ///
    /// The directory is created if it does not exist.
    pub async fn new(base_dir: impl AsRef<Path>) -> PrismResult<Self> {
        let base_dir = base_dir.as_ref().to_path_buf();
        tokio::fs::create_dir_all(&base_dir).await?;
        Ok(Self { base_dir })
    }

    fn checkpoint_path(&self, id: &str) -> PathBuf {
        self.base_dir.join(format!("{}.json", id))
    }
}

#[async_trait]
impl CheckpointStore for FileStore {
    async fn save(&self, checkpoint: &Checkpoint) -> PrismResult<()> {
        let json = checkpoint.to_json()?;
        let path = self.checkpoint_path(&checkpoint.id);
        tokio::fs::write(&path, json.as_bytes()).await?;
        Ok(())
    }

    async fn load(&self, id: &str) -> PrismResult<Checkpoint> {
        let path = self.checkpoint_path(id);
        if !path.exists() {
            return Err(PrismError::NotFound(id.to_string()));
        }
        let json = tokio::fs::read_to_string(&path).await?;
        Checkpoint::from_json(&json)
    }

    async fn list(&self, mission_id: Option<&str>) -> PrismResult<Vec<String>> {
        let mut ids = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.base_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                // Read and optionally filter by mission_id.
                let json = tokio::fs::read_to_string(&path).await?;
                match Checkpoint::from_json(&json) {
                    Ok(cp) => {
                        let include = match mission_id {
                            Some(mid) => cp.mission_id == mid,
                            None => true,
                        };
                        if include {
                            ids.push(cp.id);
                        }
                    }
                    Err(_) => {
                        // Skip malformed files.
                        continue;
                    }
                }
            }
        }
        Ok(ids)
    }

    async fn delete(&self, id: &str) -> PrismResult<()> {
        let path = self.checkpoint_path(id);
        if !path.exists() {
            return Err(PrismError::NotFound(id.to_string()));
        }
        tokio::fs::remove_file(&path).await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ReplayEngine
// ---------------------------------------------------------------------------

/// Outcome of a replay comparison.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReplayOutcome {
    /// The replayed output matched the original checkpoint output.
    Match,
    /// The replayed output was different but the agent succeeded.
    Diverged,
    /// The replay failed (agent error, timeout, etc.).
    Failed,
}

/// Detailed result of a replay operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayResult {
    /// The checkpoint that was replayed.
    pub checkpoint_id: String,
    /// The outcome of the replay.
    pub outcome: ReplayOutcome,
    /// The original response from the checkpoint.
    pub original_response: Option<AgentResponse>,
    /// The new response produced during replay.
    pub replay_response: Option<AgentResponse>,
    /// Human-readable summary of the comparison.
    pub summary: String,
}

/// Engine that replays a conversation from a checkpoint and compares outputs.
pub struct ReplayEngine {
    mesh: Arc<VotingMesh>,
}

impl ReplayEngine {
    /// Create a new replay engine that uses the given voting mesh.
    pub fn new(mesh: Arc<VotingMesh>) -> Self {
        Self { mesh }
    }

    /// Replay the conversation from the given checkpoint.
    ///
    /// The engine extracts the last user message from the checkpoint's
    /// conversation history, runs it through the voting mesh, and compares
    /// the result to the checkpoint's stored response.
    pub async fn replay(&self, checkpoint: &Checkpoint) -> PrismResult<ReplayResult> {
        // Find the last user message to use as the replay prompt.
        let last_user_message = checkpoint
            .conversation_history
            .iter()
            .rev()
            .find(|m| m.role == MessageRole::User);

        let prompt = match last_user_message {
            Some(msg) => msg.content.clone(),
            None => {
                return Ok(ReplayResult {
                    checkpoint_id: checkpoint.id.clone(),
                    outcome: ReplayOutcome::Failed,
                    original_response: checkpoint.last_response.clone(),
                    replay_response: None,
                    summary: "No user message found in checkpoint conversation history".to_string(),
                });
            }
        };

        // Run the mesh.
        let mesh_result = self.mesh.run(&prompt).await;

        match mesh_result {
            Ok(consensus) => {
                let replay_response = consensus.chosen.clone();
                let original = &checkpoint.last_response;

                let outcome = match original {
                    Some(orig) => {
                        if normalize_content(&orig.content) == normalize_content(&replay_response.content) {
                            ReplayOutcome::Match
                        } else {
                            ReplayOutcome::Diverged
                        }
                    }
                    None => {
                        // No original to compare — treat as diverged (new data).
                        ReplayOutcome::Diverged
                    }
                };

                let summary = match &outcome {
                    ReplayOutcome::Match => "Replay output matches original checkpoint".to_string(),
                    ReplayOutcome::Diverged => format!(
                        "Replay output diverged: original='{}', replay='{}'",
                        original.as_ref().map(|r| r.content.as_str()).unwrap_or("<none>"),
                        replay_response.content
                    ),
                    ReplayOutcome::Failed => unreachable!(),
                };

                Ok(ReplayResult {
                    checkpoint_id: checkpoint.id.clone(),
                    outcome,
                    original_response: original.clone(),
                    replay_response: Some(replay_response),
                    summary,
                })
            }
            Err(e) => Ok(ReplayResult {
                checkpoint_id: checkpoint.id.clone(),
                outcome: ReplayOutcome::Failed,
                original_response: checkpoint.last_response.clone(),
                replay_response: None,
                summary: format!("Replay failed: {e}"),
            }),
        }
    }
}

/// Normalize content for comparison (trim + lowercase).
fn normalize_content(s: &str) -> String {
    s.trim().to_lowercase()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{AgentEndpoint, AgentResponse, ConsensusConfig, VotingMesh};

    /// Mock agent for replay tests.
    struct ReplayMockAgent {
        response: String,
    }

    #[async_trait]
    impl AgentEndpoint for ReplayMockAgent {
        async fn generate(&self, _prompt: &str) -> PrismResult<AgentResponse> {
            Ok(AgentResponse {
                content: self.response.clone(),
                confidence: 0.95,
                model_id: "replay-mock".to_string(),
                metadata: HashMap::new(),
            })
        }
    }

    #[tokio::test]
    async fn test_checkpoint_create_and_serialize() {
        let mut cp = Checkpoint::new("mission-1");
        cp.add_message(Message::new(MessageRole::User, "Hello"));
        cp.add_message(Message::new(MessageRole::Assistant, "Hi there!"));
        cp.set_response(AgentResponse {
            content: "Hi there!".to_string(),
            confidence: 0.9,
            model_id: "test".to_string(),
            metadata: HashMap::new(),
        });

        let json = cp.to_json().unwrap();
        assert!(json.contains("mission-1"));
        assert!(json.contains("Hello"));
        assert!(json.contains("Hi there!"));

        let restored = Checkpoint::from_json(&json).unwrap();
        assert_eq!(restored.id, cp.id);
        assert_eq!(restored.mission_id, "mission-1");
        assert_eq!(restored.conversation_history.len(), 2);
        assert_eq!(restored.last_response.unwrap().content, "Hi there!");
    }

    #[tokio::test]
    async fn test_memory_store_crud() {
        let store = MemoryStore::new();

        let cp = Checkpoint::new("mission-1");
        let id = cp.id.clone();

        // Save
        store.save(&cp).await.unwrap();

        // Load
        let loaded = store.load(&id).await.unwrap();
        assert_eq!(loaded.id, id);
        assert_eq!(loaded.mission_id, "mission-1");

        // List
        let ids = store.list(None).await.unwrap();
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], id);

        // List with filter
        let ids = store.list(Some("mission-1")).await.unwrap();
        assert_eq!(ids.len(), 1);
        let ids = store.list(Some("mission-2")).await.unwrap();
        assert!(ids.is_empty());

        // Delete
        store.delete(&id).await.unwrap();
        let result = store.load(&id).await;
        assert!(matches!(result, Err(PrismError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_memory_store_not_found() {
        let store = MemoryStore::new();
        let result = store.load("nonexistent").await;
        assert!(matches!(result, Err(PrismError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_file_store_crud() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileStore::new(tmp.path()).await.unwrap();

        let mut cp = Checkpoint::new("file-mission");
        cp.add_message(Message::new(MessageRole::User, "test message"));
        let id = cp.id.clone();

        // Save
        store.save(&cp).await.unwrap();

        // Verify file exists
        let path = tmp.path().join(format!("{}.json", id));
        assert!(path.exists());

        // Load
        let loaded = store.load(&id).await.unwrap();
        assert_eq!(loaded.id, id);
        assert_eq!(loaded.mission_id, "file-mission");
        assert_eq!(loaded.conversation_history.len(), 1);

        // List
        let ids = store.list(None).await.unwrap();
        assert_eq!(ids.len(), 1);

        // Delete
        store.delete(&id).await.unwrap();
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn test_file_store_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileStore::new(tmp.path()).await.unwrap();
        let result = store.load("nonexistent").await;
        assert!(matches!(result, Err(PrismError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_replay_match() {
        let mut mesh = VotingMesh::new(ConsensusConfig::default());
        mesh.add_agent(ReplayMockAgent {
            response: "42".to_string(),
        });
        mesh.add_agent(ReplayMockAgent {
            response: "42".to_string(),
        });

        let mut cp = Checkpoint::new("replay-test");
        cp.add_message(Message::new(MessageRole::User, "What is the answer?"));
        cp.set_response(AgentResponse {
            content: "42".to_string(),
            confidence: 0.9,
            model_id: "original".to_string(),
            metadata: HashMap::new(),
        });

        let engine = ReplayEngine::new(Arc::new(mesh));
        let result = engine.replay(&cp).await.unwrap();
        assert_eq!(result.outcome, ReplayOutcome::Match);
        assert!(result.summary.contains("matches"));
    }

    #[tokio::test]
    async fn test_replay_diverged() {
        let mut mesh = VotingMesh::new(ConsensusConfig::default());
        mesh.add_agent(ReplayMockAgent {
            response: "43".to_string(),
        });
        mesh.add_agent(ReplayMockAgent {
            response: "43".to_string(),
        });

        let mut cp = Checkpoint::new("replay-diverge");
        cp.add_message(Message::new(MessageRole::User, "What is the answer?"));
        cp.set_response(AgentResponse {
            content: "42".to_string(),
            confidence: 0.9,
            model_id: "original".to_string(),
            metadata: HashMap::new(),
        });

        let engine = ReplayEngine::new(Arc::new(mesh));
        let result = engine.replay(&cp).await.unwrap();
        assert_eq!(result.outcome, ReplayOutcome::Diverged);
        assert!(result.summary.contains("diverged"));
    }

    #[tokio::test]
    async fn test_replay_no_user_message() {
        let mesh = VotingMesh::new(ConsensusConfig::default());
        let cp = Checkpoint::new("empty");

        let engine = ReplayEngine::new(Arc::new(mesh));
        let result = engine.replay(&cp).await.unwrap();
        assert_eq!(result.outcome, ReplayOutcome::Failed);
        assert!(result.summary.contains("No user message"));
    }

    #[test]
    fn test_message_builder() {
        let msg = Message::new(MessageRole::User, "hello")
            .with_name("alice");
        assert_eq!(msg.role, MessageRole::User);
        assert_eq!(msg.content, "hello");
        assert_eq!(msg.name.as_deref(), Some("alice"));
    }

    #[test]
    fn test_function_call_serialize() {
        let call = FunctionCall {
            name: "search".to_string(),
            arguments: r#"{"query": "rust"}"#.to_string(),
            result: Some("found 10 results".to_string()),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&call).unwrap();
        let restored: FunctionCall = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.name, "search");
    }
}
