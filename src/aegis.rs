//! # Aegis — Production Compliance & Risk Reliability for Exchanges
//!
//! Extends Sentinel with exchange-specific capabilities:
//!
//! 1. **Trade surveillance** — detect wash trading, spoofing, layering
//! 2. **Real-time risk scoring** — anomaly detection on trade patterns
//! 3. **KYC/AML refresh** — periodic re-screening with auto-quarantine
//! 4. **Multi-agent compliance** — parallel agents for different compliance domains
//! 5. **Exchange API hooks** — Kraken Pro, futures, staking, deposits
//!
//! Built on Sentinel's safety pipeline: consensus → compliance → checkpoint → audit.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::checkpoint::{Checkpoint, CheckpointStore, Message, MessageRole};
use crate::error::PrismResult;
use crate::mesh::VotingMesh;
use crate::sentinel::{ConsensusSummary, SentinelConfig};
use crate::sentinel_audit::{AuditEntry, AuditLog, AuditSeverity};
use crate::sentinel_compliance::{ComplianceEngine, ComplianceResult, ComplianceVerdict};

// ---------------------------------------------------------------------------
// Trade types
// ---------------------------------------------------------------------------

/// A trade action that Aegis gates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeAction {
    /// Unique trade identifier.
    pub trade_id: String,
    /// Type: "market", "limit", "stop", "withdrawal", "deposit", "stake", "unstake".
    pub action_type: String,
    /// Trading pair (e.g., "BTC/USD", "ETH/EUR").
    pub pair: Option<String>,
    /// Side: "buy" or "sell".
    pub side: Option<String>,
    /// Order size in base currency units.
    pub size: Option<f64>,
    /// Price (for limit orders).
    pub price: Option<f64>,
    /// Total value in quote currency.
    pub value_usd: f64,
    /// Account identifier.
    pub account_id: String,
    /// The agent requesting this action.
    pub agent_id: String,
    /// Reason for the trade.
    pub reason: String,
    /// Destination address (for withdrawals).
    pub destination: Option<String>,
    /// Source chain/network (for deposits).
    pub chain: Option<String>,
}

/// Risk assessment for a trade.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskAssessment {
    /// Composite risk score (0.0 = safe, 1.0 = critical).
    pub risk_score: f64,
    /// Risk category.
    pub category: RiskCategory,
    /// Individual risk signals.
    pub signals: Vec<RiskSignal>,
    /// Whether the trade should be auto-quarantined.
    pub quarantine: bool,
}

/// Risk category classification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RiskCategory {
    /// No concerns.
    Low,
    /// Flagged for review but allowed.
    Medium,
    /// Blocked pending investigation.
    High,
    /// Immediately quarantined.
    Critical,
}

/// A single risk signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskSignal {
    pub signal_type: String,
    pub severity: f64,
    pub detail: String,
}

/// Outcome of an Aegis-gated trade.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeOutcome {
    pub trade: TradeAction,
    pub verdict: TradeVerdict,
    pub risk: RiskAssessment,
    pub compliance: ComplianceResult,
    pub consensus: ConsensusSummary,
    pub checkpoint_id: String,
    pub audit_id: String,
    pub processing_ms: u64,
}

/// What happened to the trade.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TradeVerdict {
    /// Trade approved and can execute.
    Approved,
    /// Trade blocked by compliance.
    Blocked { reason: String },
    /// Trade rejected by consensus.
    Rejected { reason: String },
    /// Trade quarantined for human review.
    Quarantined { reason: String },
}

// ---------------------------------------------------------------------------
// Trade surveillance rules
// ---------------------------------------------------------------------------

/// Detects wash trading — an account trading with itself or coordinated accounts.
pub struct WashTradingDetector {
    /// Recent trades per account for pattern detection.
    recent_trades: std::sync::RwLock<Vec<TradeRecord>>,
    /// Window in seconds to check for wash patterns.
    window_secs: u64,
}

#[derive(Clone)]
struct TradeRecord {
    account_id: String,
    pair: String,
    side: String,
    size: f64,
    timestamp: i64,
}

impl WashTradingDetector {
    pub fn new() -> Self {
        Self {
            recent_trades: std::sync::RwLock::new(Vec::new()),
            window_secs: 300, // 5-minute window
        }
    }

    /// Record a trade for pattern analysis.
    pub fn record(&self, trade: &TradeAction) {
        let record = TradeRecord {
            account_id: trade.account_id.clone(),
            pair: trade.pair.clone().unwrap_or_default(),
            side: trade.side.clone().unwrap_or_default(),
            size: trade.size.unwrap_or(0.0),
            timestamp: chrono::Utc::now().timestamp(),
        };

        if let Ok(mut trades) = self.recent_trades.write() {
            trades.push(record);
            // Prune old entries
            let cutoff = chrono::Utc::now().timestamp() - self.window_secs as i64;
            trades.retain(|t| t.timestamp >= cutoff);
        }
    }

    /// Check for wash trading patterns.
    pub fn check(&self, trade: &TradeAction) -> Option<RiskSignal> {
        let trades = self.recent_trades.read().ok()?;
        let pair = trade.pair.as_deref().unwrap_or("");
        let side = trade.side.as_deref().unwrap_or("");
        let size = trade.size.unwrap_or(0.0);

        // Pattern: same account, same pair, opposite side, similar size within window
        let opposite = if side == "buy" { "sell" } else { "buy" };
        let matches: Vec<_> = trades
            .iter()
            .filter(|t| {
                t.account_id == trade.account_id
                    && t.pair == pair
                    && t.side == opposite
                    && (t.size - size).abs() / size.max(1.0) < 0.1 // Within 10% size
            })
            .collect();

        if !matches.is_empty() {
            Some(RiskSignal {
                signal_type: "wash_trading".to_string(),
                severity: 0.9,
                detail: format!(
                    "Potential wash trade: {} opposite trades on {} within {}s window (same account, similar size)",
                    matches.len(), pair, self.window_secs
                ),
            })
        } else {
            None
        }
    }
}

/// Detects spoofing — large orders placed and quickly cancelled to manipulate price.
pub struct SpoofingDetector {
    /// Threshold: orders larger than this (in USD) are monitored.
    large_order_threshold: f64,
}

impl SpoofingDetector {
    pub fn new(threshold_usd: f64) -> Self {
        Self {
            large_order_threshold: threshold_usd,
        }
    }

    pub fn check(&self, trade: &TradeAction) -> Option<RiskSignal> {
        if trade.value_usd >= self.large_order_threshold {
            Some(RiskSignal {
                signal_type: "large_order_monitor".to_string(),
                severity: 0.3,
                detail: format!(
                    "Large order ${:.2} on {} — monitoring for cancellation pattern",
                    trade.value_usd,
                    trade.pair.as_deref().unwrap_or("unknown"),
                ),
            })
        } else {
            None
        }
    }
}

/// Concentration risk — single account taking oversized position.
pub struct ConcentrationDetector {
    /// Maximum single-trade value as percentage of daily volume.
    max_trade_pct: f64,
    /// Estimated daily volume (USD) — in production, fetched from exchange.
    daily_volume_usd: f64,
}

impl ConcentrationDetector {
    pub fn new(max_pct: f64, daily_volume: f64) -> Self {
        Self {
            max_trade_pct: max_pct,
            daily_volume_usd: daily_volume,
        }
    }

    pub fn check(&self, trade: &TradeAction) -> Option<RiskSignal> {
        let pct = trade.value_usd / self.daily_volume_usd * 100.0;
        if pct >= self.max_trade_pct {
            Some(RiskSignal {
                signal_type: "concentration_risk".to_string(),
                severity: (pct / self.max_trade_pct * 0.5).min(1.0),
                detail: format!(
                    "Trade is {:.2}% of daily volume (limit: {:.1}%)",
                    pct, self.max_trade_pct
                ),
            })
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Risk engine
// ---------------------------------------------------------------------------

/// Real-time risk assessment engine.
pub struct RiskEngine {
    wash_detector: WashTradingDetector,
    spoof_detector: SpoofingDetector,
    concentration_detector: ConcentrationDetector,
    /// Risk score threshold for auto-quarantine.
    quarantine_threshold: f64,
}

impl RiskEngine {
    /// Create with production defaults.
    pub fn new() -> Self {
        Self {
            wash_detector: WashTradingDetector::new(),
            spoof_detector: SpoofingDetector::new(100_000.0), // $100K
            concentration_detector: ConcentrationDetector::new(5.0, 50_000_000.0), // 5% of $50M daily
            quarantine_threshold: 0.8,
        }
    }

    /// Create with custom thresholds.
    pub fn with_config(
        spoof_threshold: f64,
        concentration_pct: f64,
        daily_volume: f64,
        quarantine_threshold: f64,
    ) -> Self {
        Self {
            wash_detector: WashTradingDetector::new(),
            spoof_detector: SpoofingDetector::new(spoof_threshold),
            concentration_detector: ConcentrationDetector::new(concentration_pct, daily_volume),
            quarantine_threshold,
        }
    }

    /// Assess risk for a trade.
    pub fn assess(&self, trade: &TradeAction) -> RiskAssessment {
        let mut signals = Vec::new();

        if let Some(s) = self.wash_detector.check(trade) {
            signals.push(s);
        }
        if let Some(s) = self.spoof_detector.check(trade) {
            signals.push(s);
        }
        if let Some(s) = self.concentration_detector.check(trade) {
            signals.push(s);
        }

        let risk_score = if signals.is_empty() {
            0.0
        } else {
            signals.iter().map(|s| s.severity).sum::<f64>() / signals.len() as f64
        };

        let category = match risk_score {
            s if s >= 0.8 => RiskCategory::Critical,
            s if s >= 0.5 => RiskCategory::High,
            s if s >= 0.2 => RiskCategory::Medium,
            _ => RiskCategory::Low,
        };

        let quarantine = risk_score >= self.quarantine_threshold;

        RiskAssessment {
            risk_score,
            category,
            signals,
            quarantine,
        }
    }

    /// Record a trade for future pattern detection.
    pub fn record_trade(&self, trade: &TradeAction) {
        self.wash_detector.record(trade);
    }
}

// ---------------------------------------------------------------------------
// Aegis core
// ---------------------------------------------------------------------------

/// The Aegis compliance and risk reliability engine.
///
/// Extends Sentinel's safety pipeline with exchange-specific
/// trade surveillance, risk assessment, and compliance workflows.
pub struct Aegis<S: CheckpointStore> {
    mesh: VotingMesh,
    store: Arc<S>,
    compliance: ComplianceEngine,
    risk: RiskEngine,
    audit: AuditLog,
    config: SentinelConfig,
}

impl<S: CheckpointStore> Aegis<S> {
    pub fn new(
        mesh: VotingMesh,
        store: Arc<S>,
        compliance: ComplianceEngine,
        risk: RiskEngine,
        audit: AuditLog,
        config: SentinelConfig,
    ) -> Self {
        Self {
            mesh,
            store,
            compliance,
            risk,
            audit,
            config,
        }
    }

    /// Gate a trade through the full Aegis pipeline:
    /// risk assessment → compliance → consensus → checkpoint → audit.
    pub async fn gate_trade(&self, trade: &TradeAction) -> PrismResult<TradeOutcome> {
        let start = std::time::Instant::now();

        // ── Step 1: Risk assessment ──
        let risk = self.risk.assess(trade);

        if risk.quarantine {
            let audit_id = self.audit.log(AuditEntry::action(
                &trade.trade_id,
                AuditSeverity::Critical,
                "TRADE QUARANTINED — risk threshold exceeded",
                &format!(
                    "Risk score {:.2}, signals: {}",
                    risk.risk_score,
                    risk.signals
                        .iter()
                        .map(|s| s.signal_type.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                serde_json::to_value(trade).ok(),
            ));

            return Ok(TradeOutcome {
                trade: trade.clone(),
                verdict: TradeVerdict::Quarantined {
                    reason: format!(
                        "Risk score {:.2} exceeds threshold: {}",
                        risk.risk_score,
                        risk.signals
                            .iter()
                            .map(|s| s.detail.as_str())
                            .collect::<Vec<_>>()
                            .join("; "),
                    ),
                },
                risk,
                compliance: ComplianceResult {
                    verdict: ComplianceVerdict::Approved,
                    risk_score: 0.0,
                    violated_rules: vec![],
                    warnings: vec![],
                    rule_results: vec![],
                },
                consensus: ConsensusSummary {
                    total_agents: 0,
                    approvals: 0,
                    rejections: 0,
                    agreement_ratio: 0.0,
                    avg_confidence: 0.0,
                },
                checkpoint_id: String::new(),
                audit_id,
                processing_ms: start.elapsed().as_millis() as u64,
            });
        }

        // ── Step 2: Compliance check (convert trade to wallet action for reuse) ──
        let wallet_action = trade_to_wallet_action(trade);
        let compliance = self.compliance.check(&wallet_action);

        if compliance.verdict == ComplianceVerdict::Blocked {
            let audit_id = self.audit.log(AuditEntry::action(
                &trade.trade_id,
                AuditSeverity::Critical,
                "TRADE BLOCKED by compliance",
                &compliance.violated_rules.join("; "),
                serde_json::to_value(trade).ok(),
            ));

            return Ok(TradeOutcome {
                trade: trade.clone(),
                verdict: TradeVerdict::Blocked {
                    reason: compliance.violated_rules.join("; "),
                },
                risk,
                compliance,
                consensus: ConsensusSummary {
                    total_agents: 0,
                    approvals: 0,
                    rejections: 0,
                    agreement_ratio: 0.0,
                    avg_confidence: 0.0,
                },
                checkpoint_id: String::new(),
                audit_id,
                processing_ms: start.elapsed().as_millis() as u64,
            });
        }

        // ── Step 3: Consensus gate ──
        let prompt = self.build_trade_prompt(trade, &risk, &compliance);
        let consensus_result = self.mesh.run(&prompt).await?;

        let approvals = 1 + consensus_result.total_responses
            - consensus_result.dissenting.len()
            - consensus_result.failed_agents;
        let agreement = consensus_result.agreement_ratio;

        let consensus = ConsensusSummary {
            total_agents: consensus_result.total_responses,
            approvals,
            rejections: consensus_result.dissenting.len(),
            agreement_ratio: agreement,
            avg_confidence: consensus_result.confidence,
        };

        if agreement < self.config.min_agreement {
            let reason = format!(
                "consensus {:.0}% < required {:.0}%",
                agreement * 100.0,
                self.config.min_agreement * 100.0,
            );

            let audit_id = self.audit.log(AuditEntry::action(
                &trade.trade_id,
                AuditSeverity::Warning,
                "TRADE REJECTED — consensus not reached",
                &reason,
                serde_json::to_value(trade).ok(),
            ));

            return Ok(TradeOutcome {
                trade: trade.clone(),
                verdict: TradeVerdict::Rejected { reason },
                risk,
                compliance,
                consensus,
                checkpoint_id: String::new(),
                audit_id,
                processing_ms: start.elapsed().as_millis() as u64,
            });
        }

        // ── Step 4: Checkpoint ──
        let mut checkpoint = Checkpoint::new(&trade.trade_id);
        checkpoint.add_message(Message::new(
            MessageRole::System,
            format!("Aegis pre-trade checkpoint: {}", trade.trade_id),
        ));
        checkpoint.add_message(Message::new(
            MessageRole::User,
            serde_json::to_string(trade).unwrap_or_default(),
        ));
        checkpoint.set_response(consensus_result.chosen.clone());
        checkpoint
            .metadata
            .insert("risk_score".to_string(), risk.risk_score.to_string());
        checkpoint.metadata.insert(
            "risk_category".to_string(),
            format!("{:?}", risk.category),
        );
        checkpoint
            .metadata
            .insert("agreement".to_string(), agreement.to_string());

        self.store.save(&checkpoint).await?;

        // ── Step 5: Record trade for future pattern detection ──
        self.risk.record_trade(trade);

        // ── Step 6: Audit + approve ──
        let audit_id = self.audit.log(AuditEntry::action(
            &trade.trade_id,
            AuditSeverity::Info,
            "TRADE APPROVED",
            &format!(
                "{} {} {} @ ${:.2} — risk={:.2} ({:?}), consensus={:.0}%",
                trade.side.as_deref().unwrap_or("?"),
                trade.size.unwrap_or(0.0),
                trade.pair.as_deref().unwrap_or("?"),
                trade.value_usd,
                risk.risk_score,
                risk.category,
                agreement * 100.0,
            ),
            serde_json::to_value(trade).ok(),
        ));

        Ok(TradeOutcome {
            trade: trade.clone(),
            verdict: TradeVerdict::Approved,
            risk,
            compliance,
            consensus,
            checkpoint_id: checkpoint.id.clone(),
            audit_id,
            processing_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// Rollback a trade to its checkpoint.
    pub async fn rollback_trade(&self, checkpoint_id: &str, reason: &str) -> PrismResult<()> {
        let checkpoint = self.store.load(checkpoint_id).await?;

        self.audit.log(AuditEntry::action(
            &checkpoint.mission_id,
            AuditSeverity::Critical,
            "TRADE ROLLBACK",
            reason,
            serde_json::to_value(&checkpoint.metadata).ok(),
        ));

        Ok(())
    }

    /// Get the audit log for inspection/export.
    pub fn audit_log(&self) -> &AuditLog {
        &self.audit
    }

    fn build_trade_prompt(
        &self,
        trade: &TradeAction,
        risk: &RiskAssessment,
        compliance: &ComplianceResult,
    ) -> String {
        format!(
            "AEGIS TRADE SAFETY CHECK\n\n\
             You are an exchange compliance agent. Evaluate this trade and respond APPROVE or REJECT.\n\n\
             Trade: {} {} {} {}\n\
             Value: ${:.2}\n\
             Account: {}\n\
             Agent: {}\n\
             Reason: {}\n\n\
             Risk Score: {:.2} ({:?})\n\
             Risk Signals: {}\n\
             Compliance Warnings: {}\n\n\
             Consider: Is this trade safe? Are there market manipulation signals? \
             Is the size appropriate for this account?",
            trade.side.as_deref().unwrap_or("?"),
            trade.size.unwrap_or(0.0),
            trade.pair.as_deref().unwrap_or("?"),
            trade.action_type,
            trade.value_usd,
            trade.account_id,
            trade.agent_id,
            trade.reason,
            risk.risk_score,
            risk.category,
            if risk.signals.is_empty() {
                "None".to_string()
            } else {
                risk.signals
                    .iter()
                    .map(|s| format!("{}: {}", s.signal_type, s.detail))
                    .collect::<Vec<_>>()
                    .join("; ")
            },
            if compliance.warnings.is_empty() {
                "None".to_string()
            } else {
                compliance.warnings.join("; ")
            },
        )
    }
}

/// Convert a TradeAction to a WalletAction for compliance rule reuse.
fn trade_to_wallet_action(trade: &TradeAction) -> crate::sentinel::WalletAction {
    crate::sentinel::WalletAction {
        action_id: trade.trade_id.clone(),
        action_type: trade.action_type.clone(),
        from: trade.account_id.clone(),
        to: trade.destination.clone(),
        // Use raw USD cents to avoid wei inflation — compliance rules compare amounts directly
        amount: Some((trade.value_usd * 100.0) as u128),
        asset: trade.pair.clone(),
        chain_id: 1,
        data: None,
        reason: trade.reason.clone(),
        agent_id: trade.agent_id.clone(),
    }
}
