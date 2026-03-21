//! # Sentinel Compliance Engine
//!
//! Programmable compliance rules for agentic wallets.
//! Enforces AML/KYT, OFAC screening, velocity limits, and anomaly detection.
//!
//! Rules are composable — add/remove rules at runtime without recompilation.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::RwLock;

use crate::sentinel::WalletAction;

// ---------------------------------------------------------------------------
// Compliance result
// ---------------------------------------------------------------------------

/// Result of running all compliance checks on an action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceResult {
    /// Overall verdict.
    pub verdict: ComplianceVerdict,
    /// Composite risk score (0.0 = safe, 1.0 = maximum risk).
    pub risk_score: f64,
    /// Rules that were violated (if any).
    pub violated_rules: Vec<String>,
    /// Warnings that don't block but should be logged.
    pub warnings: Vec<String>,
    /// Individual rule results.
    pub rule_results: Vec<RuleResult>,
}

/// Whether the action passes compliance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ComplianceVerdict {
    /// All rules pass.
    Approved,
    /// One or more critical rules violated — action blocked.
    Blocked,
    /// Warnings present but no blocking violations.
    Warning,
}

/// Result of a single compliance rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleResult {
    pub rule_name: String,
    pub passed: bool,
    pub risk_contribution: f64,
    pub detail: String,
}

// ---------------------------------------------------------------------------
// Compliance rules
// ---------------------------------------------------------------------------

/// A single compliance rule that evaluates a wallet action.
pub trait ComplianceRule: Send + Sync {
    /// Human-readable rule name.
    fn name(&self) -> &str;

    /// Evaluate the action. Returns (passed, risk_contribution, detail).
    fn evaluate(&self, action: &WalletAction) -> (bool, f64, String);

    /// Is this rule a hard block (vs warning)?
    fn is_blocking(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Built-in rules
// ---------------------------------------------------------------------------

/// OFAC sanctions screening — blocks transactions to sanctioned addresses.
pub struct OfacScreening {
    sanctioned: HashSet<String>,
}

impl OfacScreening {
    pub fn new() -> Self {
        // In production, load from Chainalysis/Elliptic API.
        // For demo, include known sanctioned addresses.
        let mut sanctioned = HashSet::new();
        // Tornado Cash (OFAC-sanctioned)
        sanctioned.insert("0xd90e2f925da726b50c4ed8d0fb90ad053324f31b".to_lowercase());
        sanctioned.insert("0x722122df12d4e14e13ac3b6895a86e84145b6967".to_lowercase());
        // Lazarus Group wallets
        sanctioned.insert("0x098b716b8aaf21512996dc57eb0615e2383e2f96".to_lowercase());
        Self { sanctioned }
    }

    /// Add a sanctioned address at runtime.
    pub fn add_address(&mut self, address: &str) {
        self.sanctioned.insert(address.to_lowercase());
    }
}

impl ComplianceRule for OfacScreening {
    fn name(&self) -> &str {
        "OFAC Sanctions Screening"
    }

    fn evaluate(&self, action: &WalletAction) -> (bool, f64, String) {
        if let Some(ref to) = action.to {
            if self.sanctioned.contains(&to.to_lowercase()) {
                return (
                    false,
                    1.0,
                    format!("Destination {} is OFAC-sanctioned", to),
                );
            }
        }
        if self.sanctioned.contains(&action.from.to_lowercase()) {
            return (
                false,
                1.0,
                format!("Source {} is OFAC-sanctioned", action.from),
            );
        }
        (true, 0.0, "No sanctioned addresses detected".to_string())
    }
}

/// Transaction velocity limit — prevents rapid-fire spending.
pub struct VelocityLimit {
    /// Maximum actions per window.
    max_actions: u32,
    /// Window duration in seconds.
    window_secs: u64,
    /// Recent action timestamps (protected by RwLock for concurrent access).
    recent: RwLock<Vec<i64>>,
}

impl VelocityLimit {
    /// Create with default limits: 10 actions per 60 seconds.
    pub fn new() -> Self {
        Self {
            max_actions: 10,
            window_secs: 60,
            recent: RwLock::new(Vec::new()),
        }
    }

    /// Create with custom limits.
    pub fn with_limits(max_actions: u32, window_secs: u64) -> Self {
        Self {
            max_actions,
            window_secs,
            recent: RwLock::new(Vec::new()),
        }
    }

    /// Record an action timestamp.
    pub fn record_action(&self) {
        let now = chrono::Utc::now().timestamp();
        if let Ok(mut recent) = self.recent.write() {
            recent.push(now);
            // Prune old entries
            let cutoff = now - self.window_secs as i64;
            recent.retain(|&t| t >= cutoff);
        }
    }
}

impl ComplianceRule for VelocityLimit {
    fn name(&self) -> &str {
        "Transaction Velocity Limit"
    }

    fn evaluate(&self, _action: &WalletAction) -> (bool, f64, String) {
        let now = chrono::Utc::now().timestamp();
        let cutoff = now - self.window_secs as i64;

        let count = if let Ok(recent) = self.recent.read() {
            recent.iter().filter(|&&t| t >= cutoff).count() as u32
        } else {
            0
        };

        if count >= self.max_actions {
            let risk = (count as f64 / self.max_actions as f64).min(1.0);
            return (
                false,
                risk,
                format!(
                    "Velocity exceeded: {} actions in {}s (limit: {})",
                    count, self.window_secs, self.max_actions
                ),
            );
        }

        let risk = count as f64 / self.max_actions as f64 * 0.3; // Low risk if under limit
        (
            true,
            risk,
            format!(
                "Velocity OK: {}/{} actions in {}s window",
                count, self.max_actions, self.window_secs
            ),
        )
    }
}

/// Maximum single-transaction amount limit.
pub struct AmountLimit {
    /// Maximum amount per transaction (in smallest unit).
    max_amount: u128,
    /// Asset this limit applies to (None = all assets).
    asset: Option<String>,
}

impl AmountLimit {
    /// Create with limit in wei. Default: 10 ETH.
    pub fn new(max_amount: u128) -> Self {
        Self {
            max_amount,
            asset: None,
        }
    }

    /// Create with asset-specific limit.
    pub fn for_asset(asset: &str, max_amount: u128) -> Self {
        Self {
            max_amount,
            asset: Some(asset.to_string()),
        }
    }
}

impl ComplianceRule for AmountLimit {
    fn name(&self) -> &str {
        "Single Transaction Amount Limit"
    }

    fn evaluate(&self, action: &WalletAction) -> (bool, f64, String) {
        // Skip if asset doesn't match
        if let Some(ref required_asset) = self.asset {
            if action.asset.as_deref() != Some(required_asset.as_str()) {
                return (true, 0.0, "Asset not applicable".to_string());
            }
        }

        let amount = action.amount.unwrap_or(0);
        if amount > self.max_amount {
            let risk = (amount as f64 / self.max_amount as f64).min(1.0);
            return (
                false,
                risk,
                format!(
                    "Amount {} exceeds limit {} ({}x over)",
                    amount,
                    self.max_amount,
                    amount / self.max_amount.max(1),
                ),
            );
        }

        let risk = amount as f64 / self.max_amount as f64 * 0.2;
        (
            true,
            risk,
            format!("Amount OK: {} / {} limit", amount, self.max_amount),
        )
    }
}

/// Known-recipient allowlist — only approved addresses can receive funds.
pub struct AllowlistFilter {
    allowed: HashSet<String>,
}

impl AllowlistFilter {
    pub fn new() -> Self {
        Self {
            allowed: HashSet::new(),
        }
    }

    pub fn add(&mut self, address: &str) {
        self.allowed.insert(address.to_lowercase());
    }

    pub fn with_addresses(addresses: &[&str]) -> Self {
        let mut filter = Self::new();
        for addr in addresses {
            filter.add(addr);
        }
        filter
    }
}

impl ComplianceRule for AllowlistFilter {
    fn name(&self) -> &str {
        "Recipient Allowlist"
    }

    fn evaluate(&self, action: &WalletAction) -> (bool, f64, String) {
        if self.allowed.is_empty() {
            return (true, 0.0, "Allowlist disabled (empty)".to_string());
        }

        if let Some(ref to) = action.to {
            if self.allowed.contains(&to.to_lowercase()) {
                (true, 0.0, format!("{} is on allowlist", to))
            } else {
                (
                    false,
                    0.8,
                    format!("{} is NOT on allowlist ({} allowed addresses)", to, self.allowed.len()),
                )
            }
        } else {
            (true, 0.0, "No destination address".to_string())
        }
    }

    fn is_blocking(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Compliance engine
// ---------------------------------------------------------------------------

/// The compliance engine runs all registered rules against an action.
pub struct ComplianceEngine {
    rules: Vec<Box<dyn ComplianceRule>>,
}

impl ComplianceEngine {
    /// Create an empty engine.
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// Create with standard production rules.
    pub fn production() -> Self {
        let mut engine = Self::new();
        engine.add_rule(Box::new(OfacScreening::new()));
        engine.add_rule(Box::new(VelocityLimit::new()));
        engine.add_rule(Box::new(AmountLimit::new(
            10_000_000_000_000_000_000, // 10 ETH
        )));
        engine
    }

    /// Add a rule.
    pub fn add_rule(&mut self, rule: Box<dyn ComplianceRule>) {
        self.rules.push(rule);
    }

    /// Run all rules against an action.
    pub fn check(&self, action: &WalletAction) -> ComplianceResult {
        let mut rule_results = Vec::new();
        let mut violated = Vec::new();
        let mut warnings = Vec::new();
        let mut total_risk = 0.0;
        let mut blocked = false;

        for rule in &self.rules {
            let (passed, risk, detail) = rule.evaluate(action);

            rule_results.push(RuleResult {
                rule_name: rule.name().to_string(),
                passed,
                risk_contribution: risk,
                detail: detail.clone(),
            });

            total_risk += risk;

            if !passed {
                if rule.is_blocking() {
                    blocked = true;
                    violated.push(format!("{}: {}", rule.name(), detail));
                } else {
                    warnings.push(format!("{}: {}", rule.name(), detail));
                }
            }
        }

        // Normalize risk score to 0.0–1.0
        let risk_score = if self.rules.is_empty() {
            0.0
        } else {
            (total_risk / self.rules.len() as f64).min(1.0)
        };

        let verdict = if blocked {
            ComplianceVerdict::Blocked
        } else if !warnings.is_empty() {
            ComplianceVerdict::Warning
        } else {
            ComplianceVerdict::Approved
        };

        ComplianceResult {
            verdict,
            risk_score,
            violated_rules: violated,
            warnings,
            rule_results,
        }
    }
}
