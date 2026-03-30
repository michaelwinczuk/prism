//! # Sentinel Wallet — Coinbase Agentic Wallet Integration
//!
//! Client for Coinbase's Agentic Wallets and x402 payment protocol.
//! Wraps wallet creation, transaction signing, and x402 payment flows
//! behind Sentinel's safety pipeline.
//!
//! In demo mode, simulates all wallet operations locally.
//! In production mode, calls Coinbase CDP APIs.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::PrismResult;
use crate::sentinel::WalletAction;

// ---------------------------------------------------------------------------
// Wallet provider trait
// ---------------------------------------------------------------------------

/// Trait for pluggable wallet backends.
/// Implement for Coinbase CDP, local simulation, or any other provider.
#[async_trait]
pub trait WalletProvider: Send + Sync {
    /// Create a new wallet. Returns the wallet address.
    async fn create_wallet(&self, agent_id: &str) -> PrismResult<WalletInfo>;

    /// Get wallet balance for an asset.
    async fn get_balance(&self, address: &str, asset: &str) -> PrismResult<u128>;

    /// Sign and submit a transaction. Returns the tx hash.
    async fn execute(&self, action: &WalletAction) -> PrismResult<TransactionResult>;

    /// Get transaction status by hash.
    async fn get_tx_status(&self, tx_hash: &str) -> PrismResult<TransactionStatus>;
}

/// Wallet information returned on creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletInfo {
    pub address: String,
    pub agent_id: String,
    pub chain_id: u64,
    pub created_at: String,
}

/// Result of a transaction execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionResult {
    pub tx_hash: String,
    pub status: TransactionStatus,
    pub gas_used: Option<u64>,
    pub block_number: Option<u64>,
}

/// Transaction status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TransactionStatus {
    Pending,
    Confirmed,
    Failed { reason: String },
    Reverted { reason: String },
}

// ---------------------------------------------------------------------------
// x402 Payment Protocol
// ---------------------------------------------------------------------------

/// x402 payment request — what a service sends when requesting payment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct X402PaymentRequest {
    /// HTTP 402 response with payment details.
    pub amount: u128,
    pub asset: String,
    pub recipient: String,
    pub chain_id: u64,
    /// The resource URL the agent is paying to access.
    pub resource_url: String,
    /// Payment verification token.
    pub payment_token: Option<String>,
}

/// x402 payment response — what the agent sends back after paying.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct X402PaymentResponse {
    pub tx_hash: String,
    pub paid_amount: u128,
    pub asset: String,
    pub payer: String,
}

impl X402PaymentRequest {
    /// Convert an x402 payment request into a WalletAction for Sentinel to gate.
    pub fn to_wallet_action(&self, agent_id: &str, from: &str) -> WalletAction {
        WalletAction {
            action_id: format!("x402-{}", uuid::Uuid::new_v4()),
            action_type: "x402_payment".to_string(),
            from: from.to_string(),
            to: Some(self.recipient.clone()),
            amount: Some(self.amount),
            asset: Some(self.asset.clone()),
            chain_id: self.chain_id,
            data: None,
            reason: format!("x402 payment for resource: {}", self.resource_url),
            agent_id: agent_id.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Demo wallet (local simulation)
// ---------------------------------------------------------------------------

/// Simulated wallet for testing Sentinel without real blockchain.
pub struct DemoWallet {
    balances: std::sync::RwLock<HashMap<String, HashMap<String, u128>>>,
    tx_counter: std::sync::atomic::AtomicU64,
}

impl DemoWallet {
    pub fn new() -> Self {
        Self {
            balances: std::sync::RwLock::new(HashMap::new()),
            tx_counter: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Fund a demo wallet with a balance.
    pub fn fund(&self, address: &str, asset: &str, amount: u128) {
        let mut balances = self.balances.write().unwrap_or_else(|e| e.into_inner());
        balances
            .entry(address.to_string())
            .or_default()
            .insert(asset.to_string(), amount);
    }
}

#[async_trait]
impl WalletProvider for DemoWallet {
    async fn create_wallet(&self, agent_id: &str) -> PrismResult<WalletInfo> {
        let counter = self
            .tx_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let address = format!("0xdemo{:036x}", counter);

        // Give the new wallet some ETH for gas
        self.fund(&address, "ETH", 1_000_000_000_000_000_000); // 1 ETH

        Ok(WalletInfo {
            address,
            agent_id: agent_id.to_string(),
            chain_id: 8453, // Base
            created_at: chrono::Utc::now().to_rfc3339(),
        })
    }

    async fn get_balance(&self, address: &str, asset: &str) -> PrismResult<u128> {
        let balances = self.balances.read().unwrap_or_else(|e| e.into_inner());
        Ok(balances
            .get(address)
            .and_then(|assets| assets.get(asset))
            .copied()
            .unwrap_or(0))
    }

    async fn execute(&self, action: &WalletAction) -> PrismResult<TransactionResult> {
        let amount = action.amount.unwrap_or(0);
        let asset = action.asset.as_deref().unwrap_or("ETH");

        // Check balance
        let balance = self.get_balance(&action.from, asset).await?;
        if balance < amount {
            return Ok(TransactionResult {
                tx_hash: String::new(),
                status: TransactionStatus::Failed {
                    reason: format!("insufficient balance: {} < {} {}", balance, amount, asset),
                },
                gas_used: None,
                block_number: None,
            });
        }

        // Deduct from sender
        {
            let mut balances = self.balances.write().unwrap_or_else(|e| e.into_inner());
            if let Some(assets) = balances.get_mut(&action.from) {
                if let Some(bal) = assets.get_mut(asset) {
                    *bal = bal.saturating_sub(amount);
                }
            }

            // Credit to recipient
            if let Some(ref to) = action.to {
                balances
                    .entry(to.clone())
                    .or_default()
                    .entry(asset.to_string())
                    .and_modify(|b| *b += amount)
                    .or_insert(amount);
            }
        }

        let tx_num = self
            .tx_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tx_hash = format!("0x{:064x}", tx_num);

        Ok(TransactionResult {
            tx_hash,
            status: TransactionStatus::Confirmed,
            gas_used: Some(21_000),
            block_number: Some(tx_num),
        })
    }

    async fn get_tx_status(&self, _tx_hash: &str) -> PrismResult<TransactionStatus> {
        Ok(TransactionStatus::Confirmed)
    }
}
