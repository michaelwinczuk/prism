//! Integration tests for Aegis exchange compliance engine.

use async_trait::async_trait;
use prism_core::aegis::{Aegis, RiskCategory, RiskEngine, TradeAction, TradeVerdict};
use prism_core::checkpoint::MemoryStore;
use prism_core::mesh::{
    AgentEndpoint, AgentResponse, ConsensusConfig, ConsensusStrategy, VotingMesh,
};
use prism_core::sentinel::SentinelConfig;
use prism_core::sentinel_audit::AuditLog;
use prism_core::sentinel_compliance::{AmountLimit, ComplianceEngine, OfacScreening, VelocityLimit};
use std::sync::Arc;

// ── Test agents ──────────────────────────────────────────

struct ApproveBot;

#[async_trait]
impl AgentEndpoint for ApproveBot {
    async fn generate(&self, _prompt: &str) -> Result<AgentResponse, prism_core::PrismError> {
        Ok(AgentResponse {
            content: "APPROVE".to_string(),
            confidence: 0.95,
            model_id: "approve".to_string(),
            metadata: Default::default(),
        })
    }
    fn agent_id(&self) -> String { "approve".to_string() }
}

fn make_trade(id: &str, value_usd: f64) -> TradeAction {
    TradeAction {
        trade_id: id.to_string(),
        action_type: "market".to_string(),
        pair: Some("BTC/USD".to_string()),
        side: Some("buy".to_string()),
        size: Some(1.0),
        price: None,
        value_usd,
        account_id: "ACC-TEST".to_string(),
        agent_id: "test-bot".to_string(),
        reason: "test trade".to_string(),
        destination: None,
        chain: None,
    }
}

fn make_aegis() -> Aegis<MemoryStore> {
    let config = ConsensusConfig {
        strategy: ConsensusStrategy::Majority,
        min_confidence: 0.0,
        timeout_ms: 5000,
    };
    let mut mesh = VotingMesh::new(config);
    mesh.add_agent(ApproveBot);
    mesh.add_agent(ApproveBot);
    mesh.add_agent(ApproveBot);

    Aegis::new(
        mesh,
        Arc::new(MemoryStore::new()),
        ComplianceEngine::new(),
        RiskEngine::new(),
        AuditLog::new(),
        SentinelConfig::default(),
    )
}

// ══════════════════════════════════════════════════════════
// Trade pipeline tests
// ══════════════════════════════════════════════════════════

#[tokio::test]
async fn test_normal_trade_approved() {
    let aegis = make_aegis();
    let trade = make_trade("T-001", 10_000.0);

    let outcome = aegis.gate_trade(&trade).await.unwrap();
    assert_eq!(outcome.verdict, TradeVerdict::Approved);
    assert!(!outcome.checkpoint_id.is_empty());
    assert!(!outcome.audit_id.is_empty());
}

#[tokio::test]
async fn test_ofac_blocks_withdrawal() {
    let mut compliance = ComplianceEngine::new();
    compliance.add_rule(Box::new(OfacScreening::new()));

    let config = ConsensusConfig::default();
    let mut mesh = VotingMesh::new(config);
    mesh.add_agent(ApproveBot);

    let aegis = Aegis::new(
        mesh,
        Arc::new(MemoryStore::new()),
        compliance,
        RiskEngine::new(),
        AuditLog::new(),
        SentinelConfig::default(),
    );

    let trade = TradeAction {
        trade_id: "T-OFAC".to_string(),
        action_type: "withdrawal".to_string(),
        pair: None,
        side: None,
        size: None,
        price: None,
        value_usd: 5_000.0,
        account_id: "ACC-BAD".to_string(),
        agent_id: "bot".to_string(),
        reason: "withdraw".to_string(),
        destination: Some("0xd90e2f925da726b50c4ed8d0fb90ad053324f31b".to_string()),
        chain: Some("ethereum".to_string()),
    };

    let outcome = aegis.gate_trade(&trade).await.unwrap();
    assert!(matches!(outcome.verdict, TradeVerdict::Blocked { .. }));
}

// ══════════════════════════════════════════════════════════
// Risk engine tests
// ══════════════════════════════════════════════════════════

#[test]
fn test_risk_engine_low_risk_normal_trade() {
    let engine = RiskEngine::new();
    let trade = make_trade("T-LOW", 5_000.0);
    let risk = engine.assess(&trade);

    assert_eq!(risk.category, RiskCategory::Low);
    assert!(!risk.quarantine);
}

#[test]
fn test_risk_engine_concentration_risk() {
    let engine = RiskEngine::with_config(
        50_000.0,
        1.0,           // 1% of daily volume
        1_000_000.0,   // $1M daily volume
        0.8,
    );

    let trade = make_trade("T-CONC", 50_000.0); // 5% of daily volume
    let risk = engine.assess(&trade);

    assert!(risk.signals.iter().any(|s| s.signal_type == "concentration_risk"));
    assert!(risk.risk_score > 0.0);
}

#[test]
fn test_risk_engine_large_order_flagged() {
    let engine = RiskEngine::with_config(
        10_000.0, // Flag orders > $10K
        5.0,
        100_000_000.0,
        0.8,
    );

    let trade = make_trade("T-BIG", 50_000.0);
    let risk = engine.assess(&trade);

    assert!(risk.signals.iter().any(|s| s.signal_type == "large_order_monitor"));
}

#[test]
fn test_wash_trading_detection() {
    let engine = RiskEngine::new();

    // Record a sell
    let sell = TradeAction {
        trade_id: "W-SELL".to_string(),
        action_type: "limit".to_string(),
        pair: Some("ETH/USD".to_string()),
        side: Some("sell".to_string()),
        size: Some(10.0),
        price: Some(2500.0),
        value_usd: 25_000.0,
        account_id: "WASH-ACC".to_string(),
        agent_id: "bot".to_string(),
        reason: "sell".to_string(),
        destination: None,
        chain: None,
    };
    engine.record_trade(&sell);

    // Now buy the same amount — should detect wash trading
    let buy = TradeAction {
        trade_id: "W-BUY".to_string(),
        action_type: "limit".to_string(),
        pair: Some("ETH/USD".to_string()),
        side: Some("buy".to_string()),
        size: Some(10.0),
        price: Some(2495.0),
        value_usd: 24_950.0,
        account_id: "WASH-ACC".to_string(),
        agent_id: "bot".to_string(),
        reason: "buy".to_string(),
        destination: None,
        chain: None,
    };

    let risk = engine.assess(&buy);
    assert!(risk.signals.iter().any(|s| s.signal_type == "wash_trading"));
    assert!(risk.risk_score > 0.5);
}

// ══════════════════════════════════════════════════════════
// Audit trail tests
// ══════════════════════════════════════════════════════════

#[tokio::test]
async fn test_aegis_audit_chain_integrity() {
    let aegis = make_aegis();

    // Run 3 trades
    aegis.gate_trade(&make_trade("A-1", 1_000.0)).await.unwrap();
    aegis.gate_trade(&make_trade("A-2", 2_000.0)).await.unwrap();
    aegis.gate_trade(&make_trade("A-3", 3_000.0)).await.unwrap();

    let log = aegis.audit_log();
    assert!(log.len() >= 3);

    let (valid, broken) = log.verify_chain();
    assert!(valid, "audit chain broken at {:?}", broken);
}

#[tokio::test]
async fn test_aegis_checkpoint_on_approve() {
    let store = Arc::new(MemoryStore::new());

    let config = ConsensusConfig::default();
    let mut mesh = VotingMesh::new(config);
    mesh.add_agent(ApproveBot);
    mesh.add_agent(ApproveBot);

    let aegis = Aegis::new(
        mesh,
        store.clone(),
        ComplianceEngine::new(),
        RiskEngine::new(),
        AuditLog::new(),
        SentinelConfig::default(),
    );

    let trade = make_trade("CP-1", 5_000.0);
    let outcome = aegis.gate_trade(&trade).await.unwrap();

    assert_eq!(outcome.verdict, TradeVerdict::Approved);

    use prism_core::checkpoint::CheckpointStore;
    let cp = store.load(&outcome.checkpoint_id).await.unwrap();
    assert_eq!(cp.mission_id, "CP-1");
    assert!(cp.metadata.contains_key("risk_score"));
    assert!(cp.metadata.contains_key("risk_category"));
}
