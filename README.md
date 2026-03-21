# Prism Core

**Reliability layer for agentic AI systems.**

Prism provides three core capabilities for production AI agent deployments:

1. **VotingMesh** — Spawn N agents with different models, require consensus before accepting output.
2. **Checkpoint/Replay** — Snapshot agent state, replay from checkpoint for validation and recovery.
3. **Python Bindings** — `import prism_core` via PyO3 (feature-gated).

## Quick Start

### Rust

```rust
use prism_core::mesh::{VotingMesh, ConsensusConfig, AgentEndpoint, AgentResponse};
use prism_core::checkpoint::{Checkpoint, MemoryStore, CheckpointStore, Message, MessageRole};
use async_trait::async_trait;

// Implement your agent backend
struct MyAgent { model: String }

#[async_trait]
impl AgentEndpoint for MyAgent {
    async fn generate(&self, prompt: &str) -> Result<AgentResponse, prism_core::PrismError> {
        // Call your LLM here
        Ok(AgentResponse {
            content: "response".into(),
            confidence: 0.95,
            model_id: self.model.clone(),
            metadata: Default::default(),
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), prism_core::PrismError> {
    // Create a voting mesh with 3 agents
    let mut mesh = VotingMesh::new(ConsensusConfig::default());
    mesh.add_agent(MyAgent { model: "gpt-4".into() });
    mesh.add_agent(MyAgent { model: "claude-3".into() });
    mesh.add_agent(MyAgent { model: "gemini".into() });

    // Run consensus
    let result = mesh.run("What is the capital of France?").await?;
    println!("Consensus: {} (agreement: {:.0}%)", result.chosen.content, result.agreement_ratio * 100.0);

    // Checkpoint the result
    let store = MemoryStore::new();
    let mut cp = Checkpoint::new("my-mission");
    cp.add_message(Message::new(MessageRole::User, "What is the capital of France?"));
    cp.set_response(result.chosen);
    store.save(&cp).await?;

    Ok(())
}
```

### Python

```bash
# Build with Python bindings
maturin develop --features python
```

```python
import prism_core

def my_agent(prompt: str) -> dict:
    return {"content": "Paris", "confidence": 0.95, "model_id": "my-model"}

mesh = prism_core.VotingMesh(strategy="majority")
mesh.add_agent("agent-1", my_agent)
mesh.add_agent("agent-2", my_agent)
result = mesh.run("What is the capital of France?")
print(result["content"])  # "Paris"
```

## Building

```bash
# Build
cargo build --release

# Test
cargo test

# Build with Python bindings
cargo build --release --features python
```

## Architecture

```
src/
  lib.rs          # Public API, module declarations
  error.rs        # Error types
  mesh.rs         # VotingMesh, AgentEndpoint, consensus strategies
  checkpoint.rs   # Checkpoint, stores, ReplayEngine
  python.rs       # PyO3 bindings (feature-gated)
```

## License

MIT OR Apache-2.0
