//! VotingMesh: concurrent agent orchestration with configurable consensus.
//!
//! The [`VotingMesh`] spawns N agent tasks concurrently via Tokio, collects their
//! responses, and applies a consensus strategy (majority, unanimous, or weighted)
//! to select the best output.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::{PrismError, PrismResult};

// ---------------------------------------------------------------------------
// AgentEndpoint trait
// ---------------------------------------------------------------------------

/// Trait for pluggable LLM / agent backends.
///
/// Implement this trait to connect any model provider (OpenAI, Anthropic,
/// local models, mock agents for testing, etc.).
#[async_trait]
pub trait AgentEndpoint: Send + Sync {
    /// Generate a response for the given prompt.
    async fn generate(&self, prompt: &str) -> PrismResult<AgentResponse>;

    /// Return a human-readable identifier for this agent.
    fn agent_id(&self) -> String {
        "unnamed-agent".to_string()
    }
}

/// Response returned by an [`AgentEndpoint`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    /// The generated text content.
    pub content: String,
    /// Self-reported confidence score in \[0.0, 1.0\].
    pub confidence: f64,
    /// Identifier of the model that produced this response.
    pub model_id: String,
    /// Arbitrary key-value metadata.
    pub metadata: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Consensus configuration
// ---------------------------------------------------------------------------

/// Strategy used to determine consensus among agent responses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ConsensusStrategy {
    /// More than half of agents must agree.
    Majority,
    /// All agents must agree.
    Unanimous,
    /// Responses are weighted by confidence; highest weighted cluster wins.
    Weighted,
}

impl Default for ConsensusStrategy {
    fn default() -> Self {
        Self::Majority
    }
}

/// Configuration for the consensus process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusConfig {
    /// The consensus strategy to apply.
    pub strategy: ConsensusStrategy,
    /// Minimum confidence threshold; responses below this are discarded.
    pub min_confidence: f64,
    /// Maximum time (in milliseconds) to wait for all agents.
    pub timeout_ms: u64,
}

impl Default for ConsensusConfig {
    fn default() -> Self {
        Self {
            strategy: ConsensusStrategy::Majority,
            min_confidence: 0.0,
            timeout_ms: 30_000,
        }
    }
}

// ---------------------------------------------------------------------------
// ConsensusResult
// ---------------------------------------------------------------------------

/// The outcome of a consensus round.
#[must_use = "safety outcome must be checked"]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusResult {
    /// The chosen response (the consensus winner).
    pub chosen: AgentResponse,
    /// Ratio of agents that agreed with the chosen response (0.0–1.0).
    pub agreement_ratio: f64,
    /// Responses that dissented from the chosen response.
    pub dissenting: Vec<AgentResponse>,
    /// Overall confidence of the consensus (average confidence of agreeing agents).
    pub confidence: f64,
    /// Total number of agents that responded (before filtering).
    pub total_responses: usize,
    /// Number of agents that failed or timed out.
    pub failed_agents: usize,
}

// ---------------------------------------------------------------------------
// VotingMesh
// ---------------------------------------------------------------------------

/// Orchestrator that spawns N agents concurrently and applies consensus.
///
/// # Example
///
/// ```rust,no_run
/// use prism_core::mesh::{VotingMesh, ConsensusConfig};
///
/// # async fn example() -> Result<(), prism_core::error::PrismError> {
/// let mesh = VotingMesh::new(ConsensusConfig::default());
/// // mesh.add_agent(my_agent);
/// // let result = mesh.run("What is 2+2?").await?;
/// # Ok(())
/// # }
/// ```
pub struct VotingMesh {
    agents: Vec<Arc<dyn AgentEndpoint>>,
    config: ConsensusConfig,
}

impl VotingMesh {
    /// Create a new [`VotingMesh`] with the given consensus configuration.
    pub fn new(config: ConsensusConfig) -> Self {
        Self {
            agents: Vec::new(),
            config,
        }
    }

    /// Add an agent endpoint to the mesh.
    pub fn add_agent<A: AgentEndpoint + 'static>(&mut self, agent: A) {
        self.agents.push(Arc::new(agent));
    }

    /// Add a pre-wrapped `Arc<dyn AgentEndpoint>` to the mesh.
    pub fn add_agent_arc(&mut self, agent: Arc<dyn AgentEndpoint>) {
        self.agents.push(agent);
    }

    /// Return the number of agents currently registered.
    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }

    /// Return a reference to the current consensus configuration.
    pub fn config(&self) -> &ConsensusConfig {
        &self.config
    }

    /// Run all agents concurrently on the given `prompt` and apply consensus.
    ///
    /// Returns a [`ConsensusResult`] on success, or a [`PrismError`] if
    /// consensus cannot be reached.
    pub async fn run(&self, prompt: &str) -> PrismResult<ConsensusResult> {
        if self.agents.is_empty() {
            return Err(PrismError::NoAgents);
        }

        let timeout = Duration::from_millis(self.config.timeout_ms);

        // Spawn all agents concurrently.
        let mut handles = Vec::with_capacity(self.agents.len());
        for agent in &self.agents {
            let agent = Arc::clone(agent);
            let prompt = prompt.to_string();
            handles.push(tokio::spawn(async move {
                agent.generate(&prompt).await
            }));
        }

        // Collect results with timeout.
        let mut responses: Vec<AgentResponse> = Vec::with_capacity(handles.len());
        let mut failed_agents: usize = 0;

        let collection = async {
            for handle in handles {
                match handle.await {
                    Ok(Ok(response)) => responses.push(response),
                    Ok(Err(_)) => failed_agents += 1,
                    Err(_) => failed_agents += 1, // JoinError (panic, cancel)
                }
            }
        };

        if tokio::time::timeout(timeout, collection).await.is_err() {
            return Err(PrismError::Timeout {
                timeout_ms: self.config.timeout_ms,
                detail: format!(
                    "collected {}/{} responses before timeout",
                    responses.len(),
                    self.agents.len()
                ),
            });
        }

        // Filter by minimum confidence.
        let total_responses = responses.len();
        let filtered: Vec<AgentResponse> = responses
            .into_iter()
            .filter(|r| r.confidence >= self.config.min_confidence)
            .collect();

        if filtered.is_empty() {
            return Err(PrismError::ConsensusFailure {
                reason: format!(
                    "no responses met minimum confidence threshold of {}",
                    self.config.min_confidence
                ),
            });
        }

        // Apply consensus strategy.
        apply_consensus(&self.config.strategy, filtered, total_responses, failed_agents)
    }
}

// ---------------------------------------------------------------------------
// Consensus logic (private)
// ---------------------------------------------------------------------------

/// Normalize a response's content for comparison (trim whitespace, lowercase).
fn normalize(content: &str) -> String {
    content.trim().to_lowercase()
}

/// Group responses by normalized content and return groups sorted by size descending.
fn group_responses(responses: Vec<AgentResponse>) -> Vec<Vec<AgentResponse>> {
    let mut groups: HashMap<String, Vec<AgentResponse>> = HashMap::new();
    for resp in responses {
        let key = normalize(&resp.content);
        groups.entry(key).or_default().push(resp);
    }
    let mut groups: Vec<Vec<AgentResponse>> = groups.into_values().collect();
    groups.sort_by(|a, b| b.len().cmp(&a.len()));
    groups
}

fn apply_consensus(
    strategy: &ConsensusStrategy,
    responses: Vec<AgentResponse>,
    total_responses: usize,
    failed_agents: usize,
) -> PrismResult<ConsensusResult> {
    let groups = group_responses(responses);

    match strategy {
        ConsensusStrategy::Majority => consensus_majority(groups, total_responses, failed_agents),
        ConsensusStrategy::Unanimous => consensus_unanimous(groups, total_responses, failed_agents),
        ConsensusStrategy::Weighted => consensus_weighted(groups, total_responses, failed_agents),
    }
}

fn build_result(
    winning_group: Vec<AgentResponse>,
    other_groups: Vec<Vec<AgentResponse>>,
    total_responses: usize,
    failed_agents: usize,
) -> ConsensusResult {
    let agreement_count = winning_group.len();
    let agreement_ratio = if total_responses > 0 {
        agreement_count as f64 / total_responses as f64
    } else {
        0.0
    };
    let confidence = if agreement_count > 0 {
        winning_group.iter().map(|r| r.confidence).sum::<f64>() / agreement_count as f64
    } else {
        0.0
    };

    // Pick the highest-confidence response from the winning group as the chosen one.
    let chosen = winning_group
        .iter()
        .max_by(|a, b| a.confidence.partial_cmp(&b.confidence).unwrap_or(std::cmp::Ordering::Equal))
        .cloned()
        .expect("winning group is non-empty"); // safe: we only call this with non-empty groups

    let dissenting: Vec<AgentResponse> = other_groups.into_iter().flatten().collect();

    ConsensusResult {
        chosen,
        agreement_ratio,
        dissenting,
        confidence,
        total_responses,
        failed_agents,
    }
}

fn consensus_majority(
    mut groups: Vec<Vec<AgentResponse>>,
    total_responses: usize,
    failed_agents: usize,
) -> PrismResult<ConsensusResult> {
    if groups.is_empty() {
        return Err(PrismError::ConsensusFailure {
            reason: "no response groups".to_string(),
        });
    }

    let winning_group = groups.remove(0);
    let majority_threshold = (total_responses as f64 / 2.0).ceil() as usize;

    if winning_group.len() < majority_threshold {
        return Err(PrismError::ConsensusFailure {
            reason: format!(
                "largest group has {} responses but majority requires {}",
                winning_group.len(),
                majority_threshold
            ),
        });
    }

    Ok(build_result(winning_group, groups, total_responses, failed_agents))
}

fn consensus_unanimous(
    mut groups: Vec<Vec<AgentResponse>>,
    total_responses: usize,
    failed_agents: usize,
) -> PrismResult<ConsensusResult> {
    if groups.is_empty() {
        return Err(PrismError::ConsensusFailure {
            reason: "no response groups".to_string(),
        });
    }

    if groups.len() != 1 {
        return Err(PrismError::ConsensusFailure {
            reason: format!(
                "unanimous consensus requires 1 group, found {}",
                groups.len()
            ),
        });
    }

    let winning_group = groups.remove(0);
    Ok(build_result(winning_group, vec![], total_responses, failed_agents))
}

fn consensus_weighted(
    mut groups: Vec<Vec<AgentResponse>>,
    total_responses: usize,
    failed_agents: usize,
) -> PrismResult<ConsensusResult> {
    if groups.is_empty() {
        return Err(PrismError::ConsensusFailure {
            reason: "no response groups".to_string(),
        });
    }

    // Score each group by the sum of confidence values.
    let mut best_idx = 0;
    let mut best_score = 0.0_f64;
    for (i, group) in groups.iter().enumerate() {
        let score: f64 = group.iter().map(|r| r.confidence).sum();
        if score > best_score {
            best_score = score;
            best_idx = i;
        }
    }

    let winning_group = groups.remove(best_idx);
    Ok(build_result(winning_group, groups, total_responses, failed_agents))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A mock agent that returns a fixed response.
    struct MockAgent {
        id: String,
        response: String,
        confidence: f64,
    }

    impl MockAgent {
        fn new(id: &str, response: &str, confidence: f64) -> Self {
            Self {
                id: id.to_string(),
                response: response.to_string(),
                confidence,
            }
        }
    }

    #[async_trait]
    impl AgentEndpoint for MockAgent {
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

    /// A mock agent that always fails.
    struct FailingAgent;

    #[async_trait]
    impl AgentEndpoint for FailingAgent {
        async fn generate(&self, _prompt: &str) -> PrismResult<AgentResponse> {
            Err(PrismError::AgentError {
                agent_id: "failing".to_string(),
                message: "intentional failure".to_string(),
            })
        }

        fn agent_id(&self) -> String {
            "failing".to_string()
        }
    }

    #[tokio::test]
    async fn test_majority_consensus_3_agents_agree() {
        let mut mesh = VotingMesh::new(ConsensusConfig::default());
        mesh.add_agent(MockAgent::new("a", "42", 0.9));
        mesh.add_agent(MockAgent::new("b", "42", 0.85));
        mesh.add_agent(MockAgent::new("c", "43", 0.7));

        let result = mesh.run("What is the answer?").await.unwrap();
        assert_eq!(result.chosen.content, "42");
        assert!((result.agreement_ratio - 2.0 / 3.0).abs() < 0.01);
        assert_eq!(result.dissenting.len(), 1);
        assert_eq!(result.total_responses, 3);
        assert_eq!(result.failed_agents, 0);
    }

    #[tokio::test]
    async fn test_unanimous_consensus_success() {
        let config = ConsensusConfig {
            strategy: ConsensusStrategy::Unanimous,
            ..Default::default()
        };
        let mut mesh = VotingMesh::new(config);
        mesh.add_agent(MockAgent::new("a", "yes", 0.9));
        mesh.add_agent(MockAgent::new("b", "yes", 0.95));

        let result = mesh.run("agree?").await.unwrap();
        assert_eq!(result.chosen.content, "yes");
        assert!((result.agreement_ratio - 1.0).abs() < 0.01);
        assert!(result.dissenting.is_empty());
    }

    #[tokio::test]
    async fn test_unanimous_consensus_failure() {
        let config = ConsensusConfig {
            strategy: ConsensusStrategy::Unanimous,
            ..Default::default()
        };
        let mut mesh = VotingMesh::new(config);
        mesh.add_agent(MockAgent::new("a", "yes", 0.9));
        mesh.add_agent(MockAgent::new("b", "no", 0.95));

        let result = mesh.run("agree?").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_weighted_consensus() {
        let config = ConsensusConfig {
            strategy: ConsensusStrategy::Weighted,
            ..Default::default()
        };
        let mut mesh = VotingMesh::new(config);
        // One agent with very high confidence should win even if outnumbered.
        mesh.add_agent(MockAgent::new("a", "alpha", 0.99));
        mesh.add_agent(MockAgent::new("b", "beta", 0.3));
        mesh.add_agent(MockAgent::new("c", "beta", 0.3));

        let result = mesh.run("pick").await.unwrap();
        // "alpha" has weight 0.99, "beta" has weight 0.6 — alpha wins.
        assert_eq!(result.chosen.content, "alpha");
    }

    #[tokio::test]
    async fn test_no_agents_error() {
        let mesh = VotingMesh::new(ConsensusConfig::default());
        let result = mesh.run("hello").await;
        assert!(matches!(result, Err(PrismError::NoAgents)));
    }

    #[tokio::test]
    async fn test_min_confidence_filter() {
        let config = ConsensusConfig {
            min_confidence: 0.8,
            ..Default::default()
        };
        let mut mesh = VotingMesh::new(config);
        mesh.add_agent(MockAgent::new("a", "good", 0.9));
        mesh.add_agent(MockAgent::new("b", "good", 0.85));
        mesh.add_agent(MockAgent::new("c", "bad", 0.1)); // filtered out

        let result = mesh.run("test").await.unwrap();
        assert_eq!(result.chosen.content, "good");
    }

    #[tokio::test]
    async fn test_all_below_confidence_threshold() {
        let config = ConsensusConfig {
            min_confidence: 0.99,
            ..Default::default()
        };
        let mut mesh = VotingMesh::new(config);
        mesh.add_agent(MockAgent::new("a", "low", 0.5));

        let result = mesh.run("test").await;
        assert!(matches!(result, Err(PrismError::ConsensusFailure { .. })));
    }

    #[tokio::test]
    async fn test_failing_agent_counted() {
        let mut mesh = VotingMesh::new(ConsensusConfig::default());
        mesh.add_agent(MockAgent::new("a", "ok", 0.9));
        mesh.add_agent(MockAgent::new("b", "ok", 0.9));
        mesh.add_agent(FailingAgent);

        let result = mesh.run("test").await.unwrap();
        assert_eq!(result.failed_agents, 1);
        assert_eq!(result.total_responses, 2);
    }

    #[test]
    fn test_normalize() {
        assert_eq!(normalize("  Hello World  "), "hello world");
        assert_eq!(normalize("YES"), "yes");
    }

    #[test]
    fn test_group_responses() {
        let responses = vec![
            AgentResponse {
                content: "yes".to_string(),
                confidence: 0.9,
                model_id: "a".to_string(),
                metadata: HashMap::new(),
            },
            AgentResponse {
                content: "no".to_string(),
                confidence: 0.8,
                model_id: "b".to_string(),
                metadata: HashMap::new(),
            },
            AgentResponse {
                content: "Yes".to_string(),
                confidence: 0.85,
                model_id: "c".to_string(),
                metadata: HashMap::new(),
            },
        ];
        let groups = group_responses(responses);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].len(), 2); // "yes" group
        assert_eq!(groups[1].len(), 1); // "no" group
    }
}
