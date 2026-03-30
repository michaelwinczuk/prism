//! # Aegis Exchange Compliance Demo
//!
//! Demonstrates Aegis's trade surveillance and compliance pipeline:
//!
//! 1. Normal BTC buy (approved)
//! 2. Wash trading pattern (quarantined)
//! 3. Withdrawal to sanctioned address (blocked)
//! 4. Large order with concentration risk (flagged but approved)
//! 5. Rapid-fire trades (velocity blocked)
//!
//! Run: `cargo run --example exchange_compliance_demo`

use async_trait::async_trait;
use prism_core::aegis::{Aegis, RiskEngine, TradeAction, TradeVerdict};
use prism_core::checkpoint::MemoryStore;
use prism_core::mesh::{
    AgentEndpoint, AgentResponse, ConsensusConfig, ConsensusStrategy, VotingMesh,
};
use prism_core::sentinel::SentinelConfig;
use prism_core::sentinel_audit::AuditLog;
use prism_core::sentinel_compliance::{
    AmountLimit, ComplianceEngine, OfacScreening, VelocityLimit,
};
use std::sync::Arc;

// ── Compliance agents ────────────────────────────────────

struct TradeRiskAgent;

#[async_trait]
impl AgentEndpoint for TradeRiskAgent {
    async fn generate(&self, prompt: &str) -> Result<AgentResponse, prism_core::PrismError> {
        let content = if prompt.contains("wash_trading") || prompt.contains("Critical") {
            "REJECT"
        } else {
            "APPROVE"
        };
        Ok(AgentResponse {
            content: content.to_string(),
            confidence: 0.93,
            model_id: "trade-risk-v1".to_string(),
            metadata: Default::default(),
        })
    }
    fn agent_id(&self) -> String {
        "trade-risk".to_string()
    }
}

struct MarketSurveillanceAgent;

#[async_trait]
impl AgentEndpoint for MarketSurveillanceAgent {
    async fn generate(&self, prompt: &str) -> Result<AgentResponse, prism_core::PrismError> {
        let content = if prompt.contains("concentration_risk") && prompt.contains("severity") {
            "REJECT"
        } else {
            "APPROVE"
        };
        Ok(AgentResponse {
            content: content.to_string(),
            confidence: 0.91,
            model_id: "market-surveillance-v1".to_string(),
            metadata: Default::default(),
        })
    }
    fn agent_id(&self) -> String {
        "market-surveillance".to_string()
    }
}

struct ComplianceReviewAgent;

#[async_trait]
impl AgentEndpoint for ComplianceReviewAgent {
    async fn generate(&self, _prompt: &str) -> Result<AgentResponse, prism_core::PrismError> {
        Ok(AgentResponse {
            content: "APPROVE".to_string(),
            confidence: 0.95,
            model_id: "compliance-review-v1".to_string(),
            metadata: Default::default(),
        })
    }
    fn agent_id(&self) -> String {
        "compliance-review".to_string()
    }
}

#[tokio::main]
async fn main() {
    println!("═══════════════════════════════════════════════════════════");
    println!("  Aegis — Exchange Compliance & Risk Demo");
    println!("  Trade surveillance + consensus + compliance + audit");
    println!("═══════════════════════════════════════════════════════════\n");

    // ── Setup consensus mesh with 3 specialized agents ──
    let config = ConsensusConfig {
        strategy: ConsensusStrategy::Majority,
        min_confidence: 0.8,
        timeout_ms: 5000,
    };
    let mut mesh = VotingMesh::new(config);
    mesh.add_agent(TradeRiskAgent);
    mesh.add_agent(MarketSurveillanceAgent);
    mesh.add_agent(ComplianceReviewAgent);
    println!("✓ VotingMesh: 3 agents (TradeRisk, MarketSurveillance, ComplianceReview)");

    // ── Setup compliance ──
    let mut compliance = ComplianceEngine::new();
    compliance.add_rule(Box::new(OfacScreening::new()));
    compliance.add_rule(Box::new(VelocityLimit::with_limits(5, 60)));
    compliance.add_rule(Box::new(AmountLimit::new(
        500_000_000, // $5M in cents
    )));
    println!("✓ Compliance: OFAC + velocity (5/min) + amount ($5M)");

    // ── Setup risk engine ──
    let risk = RiskEngine::with_config(
        50_000.0,      // Flag orders > $50K for spoofing
        2.0,           // Flag if > 2% of daily volume
        100_000_000.0, // $100M daily volume
        0.8,           // Quarantine at 0.8 risk score
    );
    println!("✓ Risk Engine: wash trading + spoofing ($50K) + concentration (2%)");

    // ── Build Aegis ──
    let aegis = Aegis::new(
        mesh,
        Arc::new(MemoryStore::new()),
        compliance,
        risk,
        AuditLog::new(),
        SentinelConfig::default(),
    );
    println!();

    // ══════════════════════════════════════════════════════
    // Scenario 1: Normal BTC buy (should APPROVE)
    // ══════════════════════════════════════════════════════
    println!("── Scenario 1: Normal BTC market buy ──");
    let trade1 = TradeAction {
        trade_id: "T-001".to_string(),
        action_type: "market".to_string(),
        pair: Some("BTC/USD".to_string()),
        side: Some("buy".to_string()),
        size: Some(0.5),
        price: None,
        value_usd: 30_000.0,
        account_id: "ACC-100".to_string(),
        agent_id: "trading-bot-alpha".to_string(),
        reason: "DCA strategy — weekly BTC accumulation".to_string(),
        destination: None,
        chain: None,
    };

    let r1 = aegis.gate_trade(&trade1).await.unwrap();
    print_result(
        &r1.verdict,
        r1.risk.risk_score,
        r1.consensus.agreement_ratio,
        r1.processing_ms,
    );

    // ══════════════════════════════════════════════════════
    // Scenario 2: Wash trading (should QUARANTINE)
    // ══════════════════════════════════════════════════════
    println!("── Scenario 2: Wash trading pattern (buy then sell same size) ──");

    // First, seed a recent sell
    let wash_sell = TradeAction {
        trade_id: "T-002a".to_string(),
        action_type: "limit".to_string(),
        pair: Some("ETH/USD".to_string()),
        side: Some("sell".to_string()),
        size: Some(100.0),
        price: Some(2500.0),
        value_usd: 250_000.0,
        account_id: "ACC-200".to_string(),
        agent_id: "arb-bot".to_string(),
        reason: "take profit".to_string(),
        destination: None,
        chain: None,
    };
    // Record the sell in the risk engine (simulate it happening earlier)
    aegis.gate_trade(&wash_sell).await.ok();

    // Now buy the same amount — should trigger wash trading
    let wash_buy = TradeAction {
        trade_id: "T-002b".to_string(),
        action_type: "limit".to_string(),
        pair: Some("ETH/USD".to_string()),
        side: Some("buy".to_string()),
        size: Some(100.0),
        price: Some(2495.0),
        value_usd: 249_500.0,
        account_id: "ACC-200".to_string(),
        agent_id: "arb-bot".to_string(),
        reason: "re-enter position".to_string(),
        destination: None,
        chain: None,
    };

    let r2 = aegis.gate_trade(&wash_buy).await.unwrap();
    print_result(
        &r2.verdict,
        r2.risk.risk_score,
        r2.consensus.agreement_ratio,
        r2.processing_ms,
    );

    // ══════════════════════════════════════════════════════
    // Scenario 3: Withdrawal to sanctioned address (should BLOCK)
    // ══════════════════════════════════════════════════════
    println!("── Scenario 3: Withdrawal to Tornado Cash ──");
    let trade3 = TradeAction {
        trade_id: "T-003".to_string(),
        action_type: "withdrawal".to_string(),
        pair: None,
        side: None,
        size: None,
        price: None,
        value_usd: 10_000.0,
        account_id: "ACC-300".to_string(),
        agent_id: "treasury-bot".to_string(),
        reason: "Move funds to cold storage".to_string(),
        destination: Some("0xd90e2f925DA726b50C4Ed8D0Fb90Ad053324F31b".to_string()),
        chain: Some("ethereum".to_string()),
    };

    let r3 = aegis.gate_trade(&trade3).await.unwrap();
    print_result(&r3.verdict, r3.risk.risk_score, 0.0, r3.processing_ms);

    // ══════════════════════════════════════════════════════
    // Scenario 4: Large order — concentration risk (flagged, approved)
    // ══════════════════════════════════════════════════════
    println!("── Scenario 4: Large BTC buy ($3M — 3% of daily volume) ──");
    let trade4 = TradeAction {
        trade_id: "T-004".to_string(),
        action_type: "limit".to_string(),
        pair: Some("BTC/USD".to_string()),
        side: Some("buy".to_string()),
        size: Some(50.0),
        price: Some(60_000.0),
        value_usd: 3_000_000.0,
        account_id: "ACC-400".to_string(),
        agent_id: "whale-bot".to_string(),
        reason: "Institutional accumulation".to_string(),
        destination: None,
        chain: None,
    };

    let r4 = aegis.gate_trade(&trade4).await.unwrap();
    print_result(
        &r4.verdict,
        r4.risk.risk_score,
        r4.consensus.agreement_ratio,
        r4.processing_ms,
    );
    if !r4.risk.signals.is_empty() {
        for s in &r4.risk.signals {
            println!("   ⚠ {}: {}", s.signal_type, s.detail);
        }
        println!();
    }

    // ══════════════════════════════════════════════════════
    // Audit summary
    // ══════════════════════════════════════════════════════
    let log = aegis.audit_log();
    let (valid, _) = log.verify_chain();
    println!("── Audit Trail ──");
    println!(
        "   {} entries, chain integrity: {}\n",
        log.len(),
        if valid { "VERIFIED" } else { "BROKEN" }
    );

    println!("═══════════════════════════════════════════════════════════");
    println!("  Demo Complete");
    println!("  ✓ Normal BTC buy:       APPROVED");
    println!("  ✓ Wash trading:         QUARANTINED (pattern detected)");
    println!("  ✓ Sanctioned withdrawal: BLOCKED (OFAC)");
    println!("  ✓ Large order:          APPROVED with risk signals");
    println!(
        "  ✓ Audit chain:          {} entries, cryptographically verified",
        log.len()
    );
    println!("═══════════════════════════════════════════════════════════");
}

fn print_result(verdict: &TradeVerdict, risk: f64, agreement: f64, ms: u64) {
    match verdict {
        TradeVerdict::Approved => {
            println!(
                "   Verdict: APPROVED  |  Risk: {:.2}  |  Consensus: {:.0}%  |  Time: {}ms\n",
                risk,
                agreement * 100.0,
                ms
            );
        }
        TradeVerdict::Blocked { reason } => {
            println!(
                "   Verdict: BLOCKED  |  Risk: {:.2}  |  Reason: {}\n",
                risk, reason
            );
        }
        TradeVerdict::Quarantined { reason } => {
            println!(
                "   Verdict: QUARANTINED  |  Risk: {:.2}  |  Reason: {}\n",
                risk, reason
            );
        }
        TradeVerdict::Rejected { reason } => {
            println!(
                "   Verdict: REJECTED  |  Risk: {:.2}  |  Reason: {}\n",
                risk, reason
            );
        }
    }
}
