//! Travel Booking Swarm — multi-agent trip planning with consensus.
//!
//! Demonstrates VotingMesh with 3 specialized "travel agents" that recommend
//! destinations, then reach consensus on the best option.
//!
//! Run: cargo run --example travel_booking

use std::collections::HashMap;
use async_trait::async_trait;
use prism_core::prelude::*;

/// A mock travel agent that recommends based on its specialty.
struct TravelAgent {
    name: String,
    specialty: String,
    recommendation: String,
    confidence: f64,
}

#[async_trait]
impl AgentEndpoint for TravelAgent {
    async fn generate(&self, prompt: &str) -> PrismResult<AgentResponse> {
        // Simulate thinking time
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let content = format!(
            "[{}] Based on '{}', I recommend: {}. Specialty: {}",
            self.name, prompt, self.recommendation, self.specialty
        );

        Ok(AgentResponse {
            content: self.recommendation.clone(),
            confidence: self.confidence,
            model_id: self.name.clone(),
            metadata: HashMap::from([
                ("specialty".into(), self.specialty.clone()),
            ]),
        })
    }

    fn agent_id(&self) -> String {
        self.name.clone()
    }
}

#[tokio::main]
async fn main() -> Result<(), PrismError> {
    println!("=== Prism Travel Booking Swarm ===\n");

    // Create a voting mesh with majority consensus
    let config = ConsensusConfig {
        strategy: ConsensusStrategy::Majority,
        min_confidence: 0.5,
        timeout_ms: 10_000,
    };
    let mut mesh = VotingMesh::new(config);

    // Add 3 specialized travel agents
    mesh.add_agent(TravelAgent {
        name: "BudgetBot".into(),
        specialty: "budget travel".into(),
        recommendation: "Lisbon, Portugal".into(),
        confidence: 0.85,
    });
    mesh.add_agent(TravelAgent {
        name: "LuxuryAI".into(),
        specialty: "luxury experiences".into(),
        recommendation: "Lisbon, Portugal".into(),
        confidence: 0.92,
    });
    mesh.add_agent(TravelAgent {
        name: "AdventureGPT".into(),
        specialty: "adventure travel".into(),
        recommendation: "Patagonia, Argentina".into(),
        confidence: 0.78,
    });

    // Run consensus
    let prompt = "Plan a 10-day trip for a couple in March 2026, budget $5000";
    println!("Query: {}\n", prompt);

    let result = mesh.run(prompt).await?;

    println!("Consensus: {}", result.chosen.content);
    println!("Agreement: {:.0}%", result.agreement_ratio * 100.0);
    println!("Confidence: {:.0}%", result.confidence * 100.0);
    println!("Chosen by: {}", result.chosen.model_id);
    println!("Dissenting: {} agent(s)", result.dissenting.len());

    for d in &result.dissenting {
        println!("  - {} suggested: {} (confidence: {:.0}%)", d.model_id, d.content, d.confidence * 100.0);
    }

    // Checkpoint the result
    println!("\nCheckpointing result...");
    let store = MemoryStore::new();
    let mut cp = Checkpoint::new("travel-booking-001");
    cp.add_message(Message::new(MessageRole::User, prompt));
    cp.set_response(result.chosen);
    store.save(&cp).await?;

    let saved = store.list(Some("travel-booking-001")).await?;
    println!("Saved {} checkpoint(s)", saved.len());

    println!("\n=== Done ===");
    Ok(())
}
