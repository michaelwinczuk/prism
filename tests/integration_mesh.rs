//! Integration tests for VotingMesh.

use async_trait::async_trait;
use prism_core::error::{PrismError, PrismResult};
use prism_core::mesh::{
    AgentEndpoint, AgentResponse, ConsensusConfig, ConsensusStrategy, VotingMesh,
};
use std::collections::HashMap;
use std::time::Duration;

/// A deterministic mock agent.
struct DeterministicAgent {
    id: String,
    response: String,
    confidence: f64,
}

#[async_trait]
impl AgentEndpoint for DeterministicAgent {
    async fn generate(&self, _prompt: &str) -> PrismResult<AgentResponse> {
        Ok(AgentResponse {
            content: self.response.clone(),
            confidence: self.confidence,
            model_id: self.id.clone(),
            metadata: HashMap::new(),
        })
    }

    fn agent_id(&self) -> String {
        self.id.clone()
    }
}

/// A slow agent that takes longer than the timeout.
struct SlowAgent {
    delay: Duration,
}

#[async_trait]
impl AgentEndpoint for SlowAgent {
    async fn generate(&self, _prompt: &str) -> PrismResult<AgentResponse> {
        tokio::time::sleep(self.delay).await;
        Ok(AgentResponse {
            content: "slow".to_string(),
            confidence: 0.5,
            model_id: "slow".to_string(),
            metadata: HashMap::new(),
        })
    }
}

#[tokio::test]
async fn test_three_agents_majority_consensus() {
    let mut mesh = VotingMesh::new(ConsensusConfig::default());
    mesh.add_agent(DeterministicAgent {
        id: "gpt4".into(),
        response: "Paris".into(),
        confidence: 0.95,
    });
    mesh.add_agent(DeterministicAgent {
        id: "claude".into(),
        response: "Paris".into(),
        confidence: 0.92,
    });
    mesh.add_agent(DeterministicAgent {
        id: "gemini".into(),
        response: "London".into(),
        confidence: 0.6,
    });

    let result = mesh.run("What is the capital of France?").await.unwrap();

    assert_eq!(result.chosen.content, "Paris");
    assert_eq!(result.dissenting.len(), 1);
    assert_eq!(result.dissenting[0].content, "London");
    assert_eq!(result.total_responses, 3);
    assert_eq!(result.failed_agents, 0);
    assert!(result.agreement_ratio > 0.6);
    assert!(result.confidence > 0.9);
}

#[tokio::test]
async fn test_weighted_consensus_high_confidence_wins() {
    let config = ConsensusConfig {
        strategy: ConsensusStrategy::Weighted,
        min_confidence: 0.0,
        timeout_ms: 5000,
    };
    let mut mesh = VotingMesh::new(config);

    // Two low-confidence agents agree on "B", one high-confidence says "A".
    mesh.add_agent(DeterministicAgent {
        id: "expert".into(),
        response: "A".into(),
        confidence: 0.99,
    });
    mesh.add_agent(DeterministicAgent {
        id: "novice1".into(),
        response: "B".into(),
        confidence: 0.3,
    });
    mesh.add_agent(DeterministicAgent {
        id: "novice2".into(),
        response: "B".into(),
        confidence: 0.3,
    });

    let result = mesh.run("pick").await.unwrap();
    // "A" has weight 0.99, "B" has weight 0.6 — "A" wins.
    assert_eq!(result.chosen.content, "A");
}

#[tokio::test]
async fn test_no_agents_returns_error() {
    let mesh = VotingMesh::new(ConsensusConfig::default());
    let result = mesh.run("hello").await;
    assert!(matches!(result, Err(PrismError::NoAgents)));
}

#[tokio::test]
async fn test_timeout_handling() {
    let config = ConsensusConfig {
        strategy: ConsensusStrategy::Majority,
        min_confidence: 0.0,
        timeout_ms: 50, // very short timeout
    };
    let mut mesh = VotingMesh::new(config);
    mesh.add_agent(SlowAgent {
        delay: Duration::from_secs(10),
    });

    let result = mesh.run("test").await;
    assert!(matches!(result, Err(PrismError::Timeout { .. })));
}

#[tokio::test]
async fn test_five_agents_majority() {
    let mut mesh = VotingMesh::new(ConsensusConfig::default());
    for i in 0..3 {
        mesh.add_agent(DeterministicAgent {
            id: format!("agree-{i}"),
            response: "consensus".into(),
            confidence: 0.9,
        });
    }
    for i in 0..2 {
        mesh.add_agent(DeterministicAgent {
            id: format!("dissent-{i}"),
            response: "different".into(),
            confidence: 0.8,
        });
    }

    let result = mesh.run("test").await.unwrap();
    assert_eq!(result.chosen.content, "consensus");
    assert_eq!(result.dissenting.len(), 2);
    assert!((result.agreement_ratio - 0.6).abs() < 0.01);
}
