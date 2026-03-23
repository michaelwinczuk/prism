# Prism

**Experimental reliability primitives for multi-agent AI systems.**

A Rust + Tokio library with Python bindings (PyO3) for consensus-based agent orchestration, checkpointing, and replay validation. This is a research project exploring agent reliability patterns.

---

## What This Is

Prism provides building blocks for making multi-agent systems more reliable:

- **VotingMesh** — Run N agents with different models, require consensus before accepting output
- **Checkpoint/Replay** — Snapshot conversation state, replay from any point, detect output divergence
- **CodeForge** — Git-aware code generation scaffolding (clone, branch, sandbox-execute, diff)
- **MedResearch** — Citation scoring and claim extraction (experimental)
- **Python bindings** — Use from Python via PyO3

## Status

**Experimental / v0.1** — 69 tests pass, benchmarks run, the examples work. But this hasn't been used in a real production system. The Python bindings haven't been tested by real Python consumers. The benchmarks use mock agents — real LLM latency will dominate any framework overhead. Use it to learn from, prototype with, or contribute to.

## Quick Start

```rust
use prism_core::prelude::*;

let mut mesh = VotingMesh::new(ConsensusConfig::default());
mesh.add_agent(agent_1);
mesh.add_agent(agent_2);
mesh.add_agent(agent_3);

let result = mesh.run("Analyze this security vulnerability").await?;
// result.agreement_ratio, result.confidence
```

### Checkpoint/Replay

```rust
let store = FileStore::new("./checkpoints").await?;
let mut cp = Checkpoint::new("mission-001");
cp.add_message(Message::new(MessageRole::User, "Find the bug"));
cp.set_response(result.chosen);
store.save(&cp).await?;

// Replay and verify
let engine = ReplayEngine::new(Arc::new(mesh));
let replay = engine.replay(&cp).await?;
assert_eq!(replay.outcome, ReplayOutcome::Match);
```

### Python

```python
import prism_core

mesh = prism_core.VotingMesh(strategy="majority", min_confidence=0.7)
mesh.add_agent("claude", claude_fn)
mesh.add_agent("gpt4", gpt4_fn)

result = mesh.run("Is this code safe to deploy?")
```

## Benchmarks

Run with `cargo bench --bench throughput`. These use **instant mock agents** — real-world performance will be dominated by LLM API latency, not framework overhead.

| Agents | Consensus/sec (mock) | Note |
|--------|---------------------|------|
| 3 | ~215,000 | Framework overhead is negligible |
| 100 | ~15,000 | Scales well with agent count |
| 1,000 | ~1,300 | Still sub-ms per consensus |

With 200ms simulated LLM calls, 10 agents finish in ~202ms (near-perfect parallelism via Tokio).

## Project Structure

```
prism-core/
  src/
    lib.rs          # Public API
    mesh.rs         # VotingMesh, consensus strategies
    checkpoint.rs   # Checkpoint, FileStore, ReplayEngine
    codeforge.rs    # Git ops, sandbox execution, diffing
    medresearch.rs  # Evidence scoring, citation verification
    python.rs       # PyO3 bindings (feature-gated)
  examples/
    travel_booking.rs
    clinical_research.rs
  benches/
    throughput.rs
```

## Building

```bash
cargo build --release
cargo test                          # 69 tests
cargo bench --bench throughput      # benchmarks

# Python bindings
pip install maturin
maturin develop --features python
```

## How This Was Built

Built by a multi-agent AI swarm in one session. Architecture research by a 7-agent adversarial debate system (Think Tank Swarm), code generation by Claude Opus with Claude Sonnet auditing (Production Swarm). Human review took about 20 minutes. Total inference cost: $1.55.

This is an artifact of a larger system we're building — not a polished product. If you find it useful or want to improve it, PRs welcome.

## License

MIT OR Apache-2.0
