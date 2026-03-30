# Prism

[![CI](https://github.com/michaelwinczuk/prism/actions/workflows/ci.yml/badge.svg)](https://github.com/michaelwinczuk/prism/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](LICENSE)

**Reliability primitives for multi-agent AI systems.**

A Rust + Tokio library with Python bindings (PyO3) for consensus-based agent orchestration, safety pipelines, compliance enforcement, and replay validation. Framework overhead is sub-millisecond — real-world performance is dominated by LLM latency, not Prism.

---

## What This Does

Prism provides the infrastructure layer between your agents and their actions:

- **VotingMesh** — Run N agents in parallel, require consensus before accepting output (Majority, Unanimous, Weighted)
- **Sentinel** — Full safety pipeline: compliance check, consensus gate, checkpoint, execute, verify, audit
- **Aegis** — Exchange compliance engine with wash trading detection, spoofing detection, and OFAC screening
- **Checkpoint/Replay** — Snapshot conversation state, replay from any point, detect output divergence
- **CodeForge** — Git-aware code generation (clone, branch, sandbox-execute, diff)
- **MedResearch** — Citation scoring, evidence verification, claim extraction
- **Python bindings** — Use VotingMesh and Checkpoint from Python via PyO3

## Quick Start

### Consensus

```rust
use prism_core::prelude::*;

let mut mesh = VotingMesh::new(ConsensusConfig {
    strategy: ConsensusStrategy::Majority,
    min_confidence: 0.7,
    timeout_ms: 5000,
});
mesh.add_agent(agent_claude);
mesh.add_agent(agent_gpt4);
mesh.add_agent(agent_sonnet);

let result = mesh.run("Analyze this security vulnerability").await?;
// result.agreement_ratio = 1.0, result.confidence = 0.95
```

### Safety Pipeline (Sentinel)

```rust
use prism_core::sentinel::*;

// Sentinel runs the full pipeline: compliance → consensus → checkpoint → execute → verify → audit
let action = WalletAction {
    action_type: "transfer".into(),
    amount: 5000.0,
    asset: "USDC".into(),
    to: "0xabc...".into(),
    ..Default::default()
};

let outcome = sentinel.process(action).await?;
// ActionVerdict::Approved | Blocked | RolledBack | Quarantined
```

### Checkpoint/Replay

```rust
let store = FileStore::new("./checkpoints").await?;
let mut cp = Checkpoint::new("mission-001");
cp.add_message(Message::new(MessageRole::User, "Find the bug"));
cp.set_response(result.chosen);
store.save(&cp).await?;

// Replay and detect divergence
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
print(f"Agreement: {result['agreement_ratio']}")
```

## Modules

| Module | What it does |
|--------|-------------|
| **VotingMesh** | N-agent parallel consensus with configurable strategies |
| **Sentinel** | 6-stage safety pipeline (compliance → consensus → checkpoint → execute → verify → audit) |
| **Sentinel Compliance** | Pluggable rules engine — OFAC screening, velocity limits, amount limits, allowlists |
| **Sentinel Audit** | SHA-256 hash-chained tamper-evident logging |
| **Aegis** | Exchange compliance — wash trading, spoofing, layering detection + risk scoring |
| **Checkpoint** | State snapshots with MemoryStore and FileStore backends |
| **ReplayEngine** | Replay from checkpoint, detect output divergence |
| **CodeForge** | Git clone/branch/commit, sandbox execution with timeout, unified diffs |
| **MedResearch** | Evidence scoring (relevance + recency + authority), citation verification, claim extraction |
| **Semantic Eyes** | Knowledge graph traversal via mmap binary graphs |

## Benchmarks

```bash
cargo bench --bench throughput
```

Mock agents (measuring framework overhead only — real performance is LLM-bound):

| Agents | Consensus/sec | Latency |
|--------|--------------|---------|
| 3 | ~215,000 | < 5 us |
| 100 | ~15,000 | < 67 us |
| 1,000 | ~1,300 | < 770 us |

With 200ms simulated LLM calls, 10 agents finish in ~202ms (near-perfect parallelism via Tokio).

## Examples

```bash
cargo run --example travel_booking           # VotingMesh consensus demo
cargo run --example clinical_research        # MedResearch citation scoring
cargo run --example agentic_wallet_demo      # Sentinel safety pipeline (5 scenarios)
cargo run --example exchange_compliance_demo # Aegis wash trading + risk detection
```

## Project Structure

```
src/
  lib.rs                 # Public API + re-exports
  mesh.rs                # VotingMesh, consensus strategies
  checkpoint.rs          # Checkpoint, FileStore, ReplayEngine
  sentinel.rs            # Safety pipeline orchestration
  sentinel_compliance.rs # Compliance rules engine (OFAC, velocity, amount, allowlist)
  sentinel_audit.rs      # SHA-256 hash-chained audit log
  sentinel_wallet.rs     # Wallet provider trait + x402 payment protocol
  aegis.rs               # Exchange compliance (wash/spoof/layer detection)
  codeforge.rs           # Git operations, sandbox execution, diffs
  medresearch.rs         # Evidence scoring, citation verification
  semantic_eyes.rs       # Knowledge graph traversal (mmap)
  python.rs              # PyO3 bindings (feature-gated)
  error.rs               # Error types
  prelude.rs             # Convenience re-exports
tests/
  integration_mesh.rs        # 5 consensus tests
  integration_checkpoint.rs  # 6 checkpoint/replay tests
  integration_sentinel.rs    # 18 safety pipeline tests
  integration_aegis.rs       # 8 exchange compliance tests
examples/
  travel_booking.rs
  clinical_research.rs
  agentic_wallet_demo.rs
  exchange_compliance_demo.rs
benches/
  throughput.rs
  industry_benchmark.rs
```

## Building

```bash
cargo build --release
cargo test                          # 95 tests
cargo bench --bench throughput      # framework benchmarks
cargo clippy --all-targets          # lint

# Python bindings
pip install maturin
maturin develop --features python
```

## License

MIT OR Apache-2.0
