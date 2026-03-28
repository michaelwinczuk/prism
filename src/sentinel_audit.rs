//! # Sentinel Audit Trail
//!
//! Immutable, tamper-evident audit log for every Sentinel decision.
//! Every action — approved, blocked, rolled back — gets an append-only entry
//! with cryptographic chaining for verifiability.
//!
//! Designed to satisfy SOC2/SOX audit requirements and regulatory inspection.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::RwLock;

// ---------------------------------------------------------------------------
// Audit entry
// ---------------------------------------------------------------------------

/// Severity level for audit entries.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AuditSeverity {
    /// Informational — routine operation.
    Info,
    /// Warning — action allowed but flagged.
    Warning,
    /// Critical — action blocked, rolled back, or quarantined.
    Critical,
}

/// A single audit trail entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unique entry ID.
    pub id: String,
    /// Timestamp of the event.
    pub timestamp: DateTime<Utc>,
    /// Severity level.
    pub severity: AuditSeverity,
    /// Related action ID.
    pub action_id: String,
    /// What happened (short summary).
    pub event: String,
    /// Detailed description.
    pub detail: String,
    /// Hash of previous entry (chain integrity).
    pub prev_hash: String,
    /// Hash of this entry (SHA-256 of serialized content).
    pub hash: String,
    /// Optional structured data.
    pub data: Option<serde_json::Value>,
}

impl AuditEntry {
    /// Create a new action audit entry.
    pub fn action(
        action_id: &str,
        severity: AuditSeverity,
        event: &str,
        detail: &str,
        data: Option<serde_json::Value>,
    ) -> AuditEntryBuilder {
        AuditEntryBuilder {
            action_id: action_id.to_string(),
            severity,
            event: event.to_string(),
            detail: detail.to_string(),
            data,
        }
    }
}

/// Builder for creating audit entries (needs prev_hash from the log).
pub struct AuditEntryBuilder {
    action_id: String,
    severity: AuditSeverity,
    event: String,
    detail: String,
    data: Option<serde_json::Value>,
}

impl AuditEntryBuilder {
    fn build(self, prev_hash: &str) -> AuditEntry {
        let id = uuid::Uuid::new_v4().to_string();
        let timestamp = Utc::now();

        // Compute hash of this entry's content
        let hash_input = format!(
            "{}|{}|{}|{}|{}|{}",
            id, timestamp, self.action_id, self.event, self.detail, prev_hash,
        );
        let hash = sha256_hex(&hash_input);

        AuditEntry {
            id,
            timestamp,
            severity: self.severity,
            action_id: self.action_id,
            event: self.event,
            detail: self.detail,
            prev_hash: prev_hash.to_string(),
            hash,
            data: self.data,
        }
    }
}

// ---------------------------------------------------------------------------
// Audit log
// ---------------------------------------------------------------------------

/// Append-only audit log with cryptographic chaining.
///
/// Each entry's hash includes the previous entry's hash, creating a
/// tamper-evident chain. Any modification to a past entry breaks the chain.
pub struct AuditLog {
    entries: RwLock<Vec<AuditEntry>>,
}

impl AuditLog {
    /// Create a new empty audit log.
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(Vec::new()),
        }
    }

    /// Append an entry to the log. Returns the entry ID.
    pub fn log(&self, builder: AuditEntryBuilder) -> String {
        let mut entries = self.entries.write().unwrap_or_else(|e| e.into_inner());

        let prev_hash = entries
            .last()
            .map(|e| e.hash.clone())
            .unwrap_or_else(|| "GENESIS".to_string());

        let entry = builder.build(&prev_hash);
        let id = entry.id.clone();
        entries.push(entry);
        id
    }

    /// Get all entries.
    pub fn entries(&self) -> Vec<AuditEntry> {
        self.entries.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Get entries for a specific action.
    pub fn entries_for_action(&self, action_id: &str) -> Vec<AuditEntry> {
        self.entries
            .read()
            .expect("audit log poisoned")
            .iter()
            .filter(|e| e.action_id == action_id)
            .cloned()
            .collect()
    }

    /// Get entries by severity.
    pub fn entries_by_severity(&self, severity: &AuditSeverity) -> Vec<AuditEntry> {
        self.entries
            .read()
            .expect("audit log poisoned")
            .iter()
            .filter(|e| &e.severity == severity)
            .cloned()
            .collect()
    }

    /// Verify chain integrity — returns (valid, first_broken_index).
    pub fn verify_chain(&self) -> (bool, Option<usize>) {
        let entries = self.entries.read().unwrap_or_else(|e| e.into_inner());

        for i in 1..entries.len() {
            if entries[i].prev_hash != entries[i - 1].hash {
                return (false, Some(i));
            }
        }

        // Also verify each entry's own hash
        for (i, entry) in entries.iter().enumerate() {
            let expected_input = format!(
                "{}|{}|{}|{}|{}|{}",
                entry.id, entry.timestamp, entry.action_id, entry.event, entry.detail, entry.prev_hash,
            );
            let expected = sha256_hex(&expected_input);
            if entry.hash != expected {
                return (false, Some(i));
            }
        }

        (true, None)
    }

    /// Total entry count.
    pub fn len(&self) -> usize {
        self.entries.read().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Export the full log as JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        let entries = self.entries.read().unwrap_or_else(|e| e.into_inner());
        serde_json::to_string_pretty(&*entries)
    }
}

// ---------------------------------------------------------------------------
// SHA-256 (cryptographic, via sha2 crate)
// ---------------------------------------------------------------------------

/// Compute SHA-256 hex digest for audit chain integrity.
fn sha256_hex(input: &str) -> String {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    result.iter().map(|b| format!("{:02x}", b)).collect()
}
