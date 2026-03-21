# Prism

**Multi-lens reliability for agentic meshes.**

The first production reliability layer for agentic AI systems with a native **Rust + Tokio** core and **Python bindings** via PyO3. 10-50x faster than Python-native alternatives.

> Built with Claude + a production Tokio swarm in one session. The entire codebase — architecture research, code generation, audit, testing — was produced by a multi-agent AI swarm. Fork this and own your own infrastructure.

## Why Prism

Every AI agent framework handles the happy path. Prism handles everything else:

- **What happens when your agent hallucinates?** VotingMesh runs 3+ agents and requires consensus.
- **What happens when your pipeline crashes mid-run?** Checkpoint snapshots state, Replay validates recovery.
- **What happens when you need to prove your agent's reasoning?** Full audit trail with per-agent I/O tracking.
- **What happens at scale?** Native Rust + Tokio — zero GIL, true async concurrency, <2MB memory footprint.

## Core Capabilities

### VotingMesh — Consensus-driven agent orchestration
Spawn N agents with different models, require agreement before accepting output. Three strategies: **Majority**, **Unanimous**, **Weighted** (by confidence).

```rust
use prism_core::prelude::*;

let mut mesh = VotingMesh::new(ConsensusConfig::default());
mesh.add_agent(OpenAIAgent::new("gpt-4o"));
mesh.add_agent(AnthropicAgent::new("claude-sonnet"));
mesh.add_agent(GeminiAgent::new("gemini-pro"));

let result = mesh.run("Analyze this security vulnerability").await?;
// result.agreement_ratio = 0.67, result.confidence = 0.91
```

### Checkpoint/Replay — State snapshots and validation
Serialize conversation state, replay from any checkpoint, detect output divergence.

```rust
let store = FileStore::new("./checkpoints").await?;
let mut cp = Checkpoint::new("mission-001");
cp.add_message(Message::new(MessageRole::User, "Find the bug"));
cp.set_response(result.chosen);
store.save(&cp).await?;

// Later: replay and verify
let engine = ReplayEngine::new(Arc::new(mesh));
let replay = engine.replay(&cp).await?;
assert_eq!(replay.outcome, ReplayOutcome::Match);
```

### Python Bindings — Drop-in replacement for LangGraph flows
```python
import prism_core

def claude_agent(prompt: str) -> dict:
    return {"content": call_claude(prompt), "confidence": 0.95, "model_id": "claude"}

mesh = prism_core.VotingMesh(strategy="majority", min_confidence=0.7)
mesh.add_agent("claude", claude_agent)
mesh.add_agent("gpt4", gpt4_agent)
mesh.add_agent("gemini", gemini_agent)

result = mesh.run("Is this code safe to deploy?")
```

### CodeForge Pack — Git-aware code generation
Clone repos, branch, generate fixes in parallel, sandbox-execute, diff and merge.
```rust
use prism_core::codeforge::{GitSync, Sandbox, DiffEngine};

let git = GitSync::new("/tmp/my-repo");
git.init()?;
git.create_branch("fix/bug-123")?;

let sandbox = Sandbox::new(30); // 30s timeout
let result = sandbox.execute("cargo", &["test"], ".", None).await?;
println!("Tests: exit_code={:?}, timed_out={}", result.exit_code, result.timed_out);
```

### MedResearch Pack — Citation-grounded verification
Score evidence quality, verify citations, extract claims, audit cross-agent agreement.
```rust
use prism_core::medresearch::{EvidenceScorer, CitationVerifier, ClaimExtractor, ConsensusAuditor};

let score = EvidenceScorer::score(
    "aspirin reduces heart attack risk",
    "Aspirin therapy significantly reduces MI risk in high-risk patients",
    "https://pubmed.ncbi.nlm.nih.gov/12345678",
    2024,
);
// score.overall = 0.80 (high relevance + PubMed authority + recent)

let status = CitationVerifier::verify("PMID: 28724542");
// CitationStatus::Valid { format: "PubMed" }

let claims = ClaimExtractor::extract("Treatment showed 45% improvement with p < 0.05");
// [Percentage("45%"), Statistic("p < 0.05")]
```

## Benchmarks (real numbers)

Run: `cargo bench --bench throughput`

### Orchestration Throughput (instant mock agents)
| Agents | Consensus/sec | Overhead per consensus |
|--------|--------------|----------------------|
| 3 | 215,007 | 0.005ms |
| 100 | 15,218 | 0.066ms |
| 1,000 | 1,283 | 0.77ms |

### Parallel Speedup (200ms simulated LLM calls)
| Agents | Wall time | Sequential would be | Speedup |
|--------|----------|-------------------|---------|
| 3 | 214ms | 600ms | **2.8x** |
| 10 | 202ms | 2,000ms | **9.9x** |
| 50 | 203ms | 10,000ms | **49.3x** |

### vs Python frameworks
| Metric | Prism (Rust) | Python (LangGraph/CrewAI) |
|--------|-------------|--------------------------|
| Orchestration overhead | <1ms/agent | ~50ms/agent |
| 50-agent parallel | 203ms wall | GIL-serialized (~10s) |
| Memory footprint | ~2MB | ~50MB |
| Startup | <50ms | ~2s |
| Binary | <2MB | N/A (runtime) |

## Architecture

```
prism-core/
  src/
    lib.rs          # Public API + re-exports
    prelude.rs      # Convenience imports: use prism_core::prelude::*
    error.rs        # Typed errors via thiserror
    mesh.rs         # VotingMesh, AgentEndpoint trait, consensus strategies
    checkpoint.rs   # Checkpoint, MemoryStore, FileStore, ReplayEngine
    codeforge.rs    # GitSync, ParallelPR, Sandbox, DiffEngine
    medresearch.rs  # EvidenceScorer, CitationVerifier, ClaimExtractor, ConsensusAuditor
    python.rs       # PyO3 bindings (feature-gated)
  examples/
    travel_booking.rs      # Hopper-style multi-agent trip planning
    clinical_research.rs   # OpenEvidence-style medical synthesis
  benches/
    throughput.rs          # 1000-agent throughput + parallel speedup
```

## Try It

```bash
# Run the travel booking example
cargo run --example travel_booking

# Run the clinical research example
cargo run --example clinical_research

# Run benchmarks
cargo bench --bench throughput
```

## Building

```bash
# Build (release: <2MB with LTO)
cargo build --release

# Test (69 tests)
cargo test

# Build with Python bindings
pip install maturin
maturin develop --features python
```

## How This Was Built

Prism is a production artifact of a larger system: a multi-agent swarm that researches, architects, audits, tests, and ships Rust code autonomously. This is the engineering workflow I direct daily.

**Pipeline:**

| Stage | System | What It Did | Time |
|-------|--------|-------------|------|
| Architecture Research | Think Tank Swarm (TTS) | 7-agent adversarial debate across 40 knowledge clusters (26MB harvested papers). Environment Agent sets context, Alpha investigates, Omega challenges. | 93s |
| Code Generation | Production Swarm (PS) | Claude Opus architects the code, Claude Sonnet audits it, GPT-4o tests it. Per-agent I/O fully audited. | 538s |
| Domain Packs | TTS → PS Pipeline | Research → brief conversion → code gen → audit → test for CodeForge and MedResearch. | ~12min |
| Polish + Ship | Human review | Quality rules, API currency fixes, prelude module, README. | ~20min |

**Total wall-clock time:** Under 45 minutes from empty directory to public repo with 69 passing tests.

**Total inference cost:** $1.55

| Component | API Calls | Cost |
|-----------|-----------|------|
| TTS architecture research (3 missions) | 18 | ~$0.45 |
| PS code generation (3 builds) | ~15 | ~$0.90 |
| Knowledge harvesting (background) | 0 (arxiv API, free) | $0.00 |
| Semantic search indexing | 1 | ~$0.01 |
| Human engineering time | 0 API calls | $0.00 |
| **Total** | **~34 calls** | **$1.55** |

**Infrastructure:** Single desktop (i9-13900KF, 64GB DDR5). No cloud. No containers. No Kubernetes. One Rust binary per swarm, Tokio async tasks, file-based IPC.

This isn't a demo — it's how I build production systems. The swarm that built Prism runs the same pipeline for every project: deterministic knowledge retrieval, adversarial validation, automated code generation with audit trails, and continuous knowledge harvesting that makes every subsequent build smarter.

**Fork this and own your own infrastructure.**

## License

MIT OR Apache-2.0
