//! Integration tests for Checkpoint and Replay.

use async_trait::async_trait;
use prism_core::checkpoint::{
    Checkpoint, CheckpointStore, FileStore, MemoryStore, Message, MessageRole, ReplayEngine,
    ReplayOutcome,
};
use prism_core::error::PrismResult;
use prism_core::mesh::{AgentEndpoint, AgentResponse, ConsensusConfig, VotingMesh};
use std::collections::HashMap;
use std::sync::Arc;

struct FixedAgent {
    response: String,
}

#[async_trait]
impl AgentEndpoint for FixedAgent {
    async fn generate(&self, _prompt: &str) -> PrismResult<AgentResponse> {
        Ok(AgentResponse {
            content: self.response.clone(),
            confidence: 0.95,
            model_id: "fixed".to_string(),
            metadata: HashMap::new(),
        })
    }
}

#[tokio::test]
async fn test_checkpoint_roundtrip_memory_store() {
    let store = MemoryStore::new();

    // Create a checkpoint with conversation history.
    let mut cp = Checkpoint::new("integration-test");
    cp.add_message(Message::new(
        MessageRole::System,
        "You are a helpful assistant.",
    ));
    cp.add_message(Message::new(MessageRole::User, "What is Rust?"));
    cp.add_message(Message::new(
        MessageRole::Assistant,
        "Rust is a systems programming language.",
    ));
    cp.set_response(AgentResponse {
        content: "Rust is a systems programming language.".to_string(),
        confidence: 0.95,
        model_id: "test-model".to_string(),
        metadata: HashMap::new(),
    });

    let id = cp.id.clone();

    // Save and reload.
    store.save(&cp).await.unwrap();
    let loaded = store.load(&id).await.unwrap();

    assert_eq!(loaded.id, id);
    assert_eq!(loaded.mission_id, "integration-test");
    assert_eq!(loaded.conversation_history.len(), 3);
    assert_eq!(
        loaded.last_response.as_ref().unwrap().content,
        "Rust is a systems programming language."
    );
}

#[tokio::test]
async fn test_checkpoint_roundtrip_file_store() {
    let tmp = tempfile::tempdir().unwrap();
    let store = FileStore::new(tmp.path()).await.unwrap();

    let mut cp = Checkpoint::new("file-test");
    cp.add_message(Message::new(MessageRole::User, "Hello"));
    let id = cp.id.clone();

    store.save(&cp).await.unwrap();
    let loaded = store.load(&id).await.unwrap();
    assert_eq!(loaded.id, id);
    assert_eq!(loaded.conversation_history.len(), 1);

    // List and delete.
    let ids = store.list(Some("file-test")).await.unwrap();
    assert_eq!(ids.len(), 1);

    store.delete(&id).await.unwrap();
    let ids = store.list(None).await.unwrap();
    assert!(ids.is_empty());
}

#[tokio::test]
async fn test_replay_produces_match() {
    let mut mesh = VotingMesh::new(ConsensusConfig::default());
    mesh.add_agent(FixedAgent {
        response: "42".to_string(),
    });
    mesh.add_agent(FixedAgent {
        response: "42".to_string(),
    });

    let mut cp = Checkpoint::new("replay-match");
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
    assert_eq!(result.checkpoint_id, cp.id);
    assert!(result.replay_response.is_some());
}

#[tokio::test]
async fn test_replay_detects_divergence() {
    let mut mesh = VotingMesh::new(ConsensusConfig::default());
    mesh.add_agent(FixedAgent {
        response: "new-answer".to_string(),
    });
    mesh.add_agent(FixedAgent {
        response: "new-answer".to_string(),
    });

    let mut cp = Checkpoint::new("replay-diverge");
    cp.add_message(Message::new(MessageRole::User, "What is the answer?"));
    cp.set_response(AgentResponse {
        content: "old-answer".to_string(),
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
async fn test_replay_handles_empty_conversation() {
    let mesh = VotingMesh::new(ConsensusConfig::default());
    let cp = Checkpoint::new("empty");

    let engine = ReplayEngine::new(Arc::new(mesh));
    let result = engine.replay(&cp).await.unwrap();

    assert_eq!(result.outcome, ReplayOutcome::Failed);
    assert!(result.summary.contains("No user message"));
}

#[tokio::test]
async fn test_multiple_checkpoints_same_mission() {
    let store = MemoryStore::new();

    let cp1 = Checkpoint::new("shared-mission");
    let cp2 = Checkpoint::new("shared-mission");
    let cp3 = Checkpoint::new("other-mission");

    store.save(&cp1).await.unwrap();
    store.save(&cp2).await.unwrap();
    store.save(&cp3).await.unwrap();

    let all = store.list(None).await.unwrap();
    assert_eq!(all.len(), 3);

    let shared = store.list(Some("shared-mission")).await.unwrap();
    assert_eq!(shared.len(), 2);

    let other = store.list(Some("other-mission")).await.unwrap();
    assert_eq!(other.len(), 1);
}
