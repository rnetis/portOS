//! Audit trail records (PRD F-40): every user and system action that
//! changes state is appended here — write-once, queryable.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One audited action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRecord {
    pub id: Uuid,
    pub at: DateTime<Utc>,
    /// Username, service account id, or `system:<component>`.
    pub actor: String,
    /// Verb, e.g. `gate.tx.start`, `invoice.issue`, `berth.allocate`.
    pub action: String,
    /// Entity kind, e.g. `gate_transaction`.
    pub entity_type: String,
    /// Entity id or natural key.
    pub entity_id: String,
    /// Structured before/after or context details.
    pub details: Option<serde_json::Value>,
}

impl AuditRecord {
    #[must_use]
    pub fn system(action: &str, entity_type: &str, entity_id: &str) -> Self {
        Self {
            id: Uuid::now_v7(),
            at: Utc::now(),
            actor: "system".into(),
            action: action.to_string(),
            entity_type: entity_type.to_string(),
            entity_id: entity_id.to_string(),
            details: None,
        }
    }
}
