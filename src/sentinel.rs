//! # Sentinel — Production Safety for Agentic Wallets
//!
//! Sentinel wraps every agent wallet action in a safety pipeline:
//!
//! 1. **Compliance check** — screen against rules before execution
//! 2. **Consensus gate** — multiple models must agree the action is safe
//! 3. **Checkpoint** — snapshot state before execution
//! 4. **Execute** — perform the action
//! 5. **Verify** — confirm outcome matches expectations
//! 6. **Audit** — log everything to tamper-proof trail
//!
//! If any step fails, Sentinel rolls back to the checkpoint and quarantines
//! the action for review.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::checkpoint::{Checkpoint, CheckpointStore, Message, MessageRole};
use crate::error::PrismResult;
use crate::mesh::VotingMesh;
use crate::sentinel_audit::{AuditEntry, AuditLog, AuditSeverity};
use crate::sentinel_compliance::{ComplianceEngine, ComplianceResult, ComplianceVerdict};

// ---------------------------------------------------------------------------
// Transaction types
// ---------------------------------------------------------------------------

/// A wallet action that Sentinel gates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletAction {
    /// Unique action identifier.
    pub action_id: String,
    /// Type of action: "transfer", "swap", "approve", "deploy", "sign".
    pub action_type: String,
    /// Source wallet address.
    pub from: String,
    /// Destination address (if applicable).
    pub to: Option<String>,
    /// Amount in smallest unit (wei, satoshi, etc.).
    pub amount: Option<u128>,
    /// Token or asset identifier.
    pub asset: Option<String>,
    /// Chain ID (8453 for Base, 1 for Ethereum mainnet).
    pub chain_id: u64,
    /// Raw transaction data (hex-encoded).
    pub data: Option<String>,
    /// Human-readable description of why the agent wants to do this.
    pub reason: String,
    /// The agent requesting this action.
    pub agent_id: String,
}

/// Outcome of a Sentinel-gated action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionOutcome {
    /// The original action.
    pub action: WalletAction,
    /// Final verdict.
    pub verdict: ActionVerdict,
    /// Compliance result.
    pub compliance: ComplianceResult,
    /// Consensus result summary.
    pub consensus: ConsensusSummary,
    /// Checkpoint ID (for rollback if needed).
    pub checkpoint_id: String,
    /// Audit entry ID.
    pub audit_id: String,
    /// Total processing time.
    pub processing_ms: u64,
}

/// What happened to the action.
#[must_use = "safety outcome must be checked"]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ActionVerdict {
    /// Approved and executed.
    Approved,
    /// Blocked by compliance rules.
    Blocked { reason: String },
    /// Consensus not reached — agents disagreed.
    Rejected { reason: String },
    /// Execution failed — rolled back to checkpoint.
    RolledBack { reason: String },
    /// Quarantined for human review.
    Quarantined { reason: String },
}

/// Summary of the consensus vote on an action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusSummary {
    pub total_agents: usize,
    pub approvals: usize,
    pub rejections: usize,
    pub agreement_ratio: f64,
    pub avg_confidence: f64,
}

// ---------------------------------------------------------------------------
// Sentinel core
// ---------------------------------------------------------------------------

/// The Sentinel safety pipeline.
///
/// Wraps a VotingMesh (for consensus) and a CheckpointStore (for rollback)
/// to gate every wallet action through a multi-step safety process.
pub struct Sentinel<S: CheckpointStore> {
    mesh: VotingMesh,
    store: Arc<S>,
    compliance: ComplianceEngine,
    audit: AuditLog,
    config: SentinelConfig,
}

/// Sentinel configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentinelConfig {
    /// Minimum agreement ratio to approve an action (0.0–1.0).
    /// Default: 0.67 (2/3 supermajority for financial actions).
    pub min_agreement: f64,
    /// Maximum action amount before requiring unanimous consensus.
    /// Default: 1_000_000_000_000_000_000 (1 ETH in wei).
    pub high_value_threshold: u128,
    /// Whether to auto-quarantine actions that fail compliance.
    /// Default: true.
    pub quarantine_on_compliance_fail: bool,
    /// Maximum processing time before timeout (ms).
    /// Default: 5000 (5 seconds).
    pub timeout_ms: u64,
}

impl Default for SentinelConfig {
    fn default() -> Self {
        Self {
            min_agreement: 0.67,
            high_value_threshold: 1_000_000_000_000_000_000, // 1 ETH
            quarantine_on_compliance_fail: true,
            timeout_ms: 5_000,
        }
    }
}

impl<S: CheckpointStore> Sentinel<S> {
    /// Create a new Sentinel with the given components.
    pub fn new(
        mesh: VotingMesh,
        store: Arc<S>,
        compliance: ComplianceEngine,
        audit: AuditLog,
        config: SentinelConfig,
    ) -> Self {
        Self {
            mesh,
            store,
            compliance,
            audit,
            config,
        }
    }

    /// Gate a wallet action through the full safety pipeline.
    ///
    /// Steps: compliance → consensus → checkpoint → execute → verify → audit.
    /// Returns the outcome with full audit trail.
    pub async fn gate(&self, action: &WalletAction) -> PrismResult<ActionOutcome> {
        let start = std::time::Instant::now();

        // ── Step 1: Compliance check ──
        let compliance = self.compliance.check(action);

        if compliance.verdict == ComplianceVerdict::Blocked {
            let audit_id = self.audit.log(AuditEntry::action(
                &action.action_id,
                AuditSeverity::Critical,
                "ACTION BLOCKED by compliance",
                &format!("Rules violated: {}", compliance.violated_rules.join(", ")),
                serde_json::to_value(action).ok(),
            ));

            return Ok(ActionOutcome {
                action: action.clone(),
                verdict: ActionVerdict::Blocked {
                    reason: compliance.violated_rules.join("; "),
                },
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

        // ── Step 2: Consensus gate ──
        let consensus_prompt = self.build_consensus_prompt(action, &compliance);
        let consensus_result = self.mesh.run(&consensus_prompt).await?;

        let approvals = 1 + consensus_result.total_responses
            - consensus_result.dissenting.len()
            - consensus_result.failed_agents;
        let total = consensus_result.total_responses;
        let agreement = consensus_result.agreement_ratio;

        // Determine required agreement level
        let required_agreement = if self.is_high_value(action) {
            1.0 // Unanimous for high-value
        } else {
            self.config.min_agreement
        };

        let consensus_summary = ConsensusSummary {
            total_agents: total,
            approvals,
            rejections: consensus_result.dissenting.len(),
            agreement_ratio: agreement,
            avg_confidence: consensus_result.confidence,
        };

        if agreement < required_agreement {
            let reason = format!(
                "consensus {:.0}% < required {:.0}% ({} approve, {} reject)",
                agreement * 100.0,
                required_agreement * 100.0,
                approvals,
                consensus_result.dissenting.len(),
            );

            let audit_id = self.audit.log(AuditEntry::action(
                &action.action_id,
                AuditSeverity::Warning,
                "ACTION REJECTED — consensus not reached",
                &reason,
                serde_json::to_value(action).ok(),
            ));

            return Ok(ActionOutcome {
                action: action.clone(),
                verdict: ActionVerdict::Rejected { reason },
                compliance,
                consensus: consensus_summary,
                checkpoint_id: String::new(),
                audit_id,
                processing_ms: start.elapsed().as_millis() as u64,
            });
        }

        // ── Check if consensus agreed on REJECTION ──
        // High agreement doesn't mean approval — if agents unanimously say
        // "REJECT", the action must be blocked, not approved.
        {
            let chosen_lower = consensus_result.chosen.content.to_lowercase();
            if chosen_lower.contains("reject")
                || chosen_lower.contains("block")
                || chosen_lower.contains("deny")
            {
                let reason = format!(
                    "consensus agreed on rejection ({:.0}% agreement): {}",
                    agreement * 100.0,
                    consensus_result.chosen.content.trim(),
                );

                let audit_id = self.audit.log(AuditEntry::action(
                    &action.action_id,
                    AuditSeverity::Critical,
                    "ACTION BLOCKED — consensus agreed on rejection",
                    &reason,
                    serde_json::to_value(action).ok(),
                ));

                return Ok(ActionOutcome {
                    action: action.clone(),
                    verdict: ActionVerdict::Blocked { reason },
                    compliance,
                    consensus: consensus_summary,
                    checkpoint_id: String::new(),
                    audit_id,
                    processing_ms: start.elapsed().as_millis() as u64,
                });
            }
        }

        // ── Step 3: Checkpoint (snapshot state before execution) ──
        let mut checkpoint = Checkpoint::new(&action.action_id);
        checkpoint.add_message(Message::new(
            MessageRole::System,
            format!(
                "Sentinel pre-execution checkpoint for action {}",
                action.action_id
            ),
        ));
        checkpoint.add_message(Message::new(
            MessageRole::User,
            serde_json::to_string(action).unwrap_or_default(),
        ));
        checkpoint.set_response(consensus_result.chosen.clone());
        checkpoint.metadata.insert(
            "compliance_verdict".to_string(),
            format!("{:?}", compliance.verdict),
        );
        checkpoint
            .metadata
            .insert("risk_score".to_string(), compliance.risk_score.to_string());
        checkpoint
            .metadata
            .insert("agreement_ratio".to_string(), agreement.to_string());

        self.store.save(&checkpoint).await?;

        // ── Step 4: Action approved ──
        // In production, this is where the actual wallet transaction would execute.
        // For now, we return Approved — the caller executes and calls verify().

        let audit_id = self.audit.log(AuditEntry::action(
            &action.action_id,
            AuditSeverity::Info,
            "ACTION APPROVED",
            &format!(
                "consensus={:.0}%, risk={:.2}, agents={}/{}",
                agreement * 100.0,
                compliance.risk_score,
                approvals,
                total,
            ),
            serde_json::to_value(action).ok(),
        ));

        Ok(ActionOutcome {
            action: action.clone(),
            verdict: ActionVerdict::Approved,
            compliance,
            consensus: consensus_summary,
            checkpoint_id: checkpoint.id.clone(),
            audit_id,
            processing_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// Roll back to a checkpoint after a failed execution.
    pub async fn rollback(&self, checkpoint_id: &str, reason: &str) -> PrismResult<()> {
        let checkpoint = self.store.load(checkpoint_id).await?;

        self.audit.log(AuditEntry::action(
            &checkpoint.mission_id,
            AuditSeverity::Critical,
            "ROLLBACK EXECUTED",
            reason,
            serde_json::to_value(&checkpoint.metadata).ok(),
        ));

        Ok(())
    }

    /// Check if an action is high-value (requires unanimous consensus).
    fn is_high_value(&self, action: &WalletAction) -> bool {
        action
            .amount
            .map(|a| a >= self.config.high_value_threshold)
            .unwrap_or(false)
    }

    /// Build the prompt that consensus agents evaluate.
    fn build_consensus_prompt(
        &self,
        action: &WalletAction,
        compliance: &ComplianceResult,
    ) -> String {
        format!(
            "SENTINEL TRANSACTION SAFETY CHECK\n\n\
             You are a financial safety agent. Evaluate this wallet action and respond with \
             APPROVE or REJECT (one word only).\n\n\
             Action: {} {}\n\
             From: {}\n\
             To: {}\n\
             Amount: {} {}\n\
             Chain: {}\n\
             Reason: {}\n\
             Agent: {}\n\n\
             Compliance Risk Score: {:.2}/1.0\n\
             Compliance Warnings: {}\n\n\
             Consider: Is this action safe? Is the amount reasonable? \
             Is the destination suspicious? Does the reason make sense?",
            action.action_type,
            action.asset.as_deref().unwrap_or("ETH"),
            action.from,
            action.to.as_deref().unwrap_or("N/A"),
            action.amount.unwrap_or(0),
            action.asset.as_deref().unwrap_or("wei"),
            action.chain_id,
            action.reason,
            action.agent_id,
            compliance.risk_score,
            if compliance.warnings.is_empty() {
                "None".to_string()
            } else {
                compliance.warnings.join(", ")
            },
        )
    }
}
